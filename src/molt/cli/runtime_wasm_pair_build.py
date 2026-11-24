from __future__ import annotations

from molt.cli import progress as _progress
from molt.cli.build_locks import (
    BuildLockAcquisitionError,
    _build_lock,
)

import contextlib
import json
import subprocess
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Literal

from molt.cargo_execution_policy import source_build_disabled_reason
from molt.cli.artifact_state import (
    _build_state_root,
    _runtime_target_fingerprint_path,
)
from molt.cli.cargo_build_reads import cargo_build_reads, uncovered_build_inputs
from molt.cli.runtime_source_closure import runtime_source_paths
from molt.cli.runtime_build_identity import (
    RuntimeBuildInputCapture,
    RuntimeBuildInputObservation,
    runtime_source_roots,
)
from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
from molt.cli.cargo_execution import (
    CargoPlanExecutionError,
    _text_output,
    cargo_execution_evidence,
)
from molt.cli.config_resolution import (
    DEFAULT_RUNTIME_STDLIB_PROFILE,
)
from molt.cli.installed_runtime import (
    InstalledRuntimeCell,
    admit_installed_wasm_runtime,
    reuse_installed_wasm_generation,
    select_installed_wasm_runtime,
)
from molt.cli.models import (
    _RuntimeArtifactState,
)
from molt.cli.runtime_artifact_selection import (
    RuntimeCrateType,
)
from molt.cli.runtime_identity_schema import (
    RuntimeBuildIdentity,
    RuntimeToolchainContentManifest,
    runtime_build_fingerprint,
)
from molt.cli.runtime_fingerprints import (
    _write_runtime_fingerprint,
)
from molt.cli.build_request import BuildProfile
from molt.cli.runtime_wasm_build import _materialize_runtime_wasm_member_from_target
from molt.cli.runtime_wasm_build_spec import (
    _compute_runtime_wasm_build_spec,
    _resolve_runtime_wasm_cargo_specs,
    _resolved_runtime_wasm_family_identities,
    _runtime_source_identity_tree,
    _RuntimeWasmBuildSpec,
    _timed_runtime_identity_phase,
)
from molt.cli.runtime_wasm_build_support import (
    RuntimeWasmLinkError,
    _current_runtime_target_artifact,
    _reported_runtime_artifacts_from_cargo_stdout,
    _run_runtime_wasm_cargo_build,
    _wasm_cargo_toolchain_input_paths,
    _wasm_runtime_staticlib_candidates,
    _wasm_runtime_wasm_candidates,
)
from molt.cli.runtime_wasm_build_timings import (
    _record_runtime_wasm_build_phase,
)
from molt.cli.product_store import ProductStore, ProductStoreError
from molt.cli.runtime_wasm_failure import record_runtime_wasm_failure
from molt.cli.runtime_wasm_generation import (
    RuntimeWasmCodegenUse,
    RuntimeWasmGeneration,
    RuntimeWasmRelocGeneration,
    SourceLinkedRuntimeWasmGeneration,
    SourceRuntimeWasmGeneration,
    SourceWasmGeneration,
    bind_runtime_wasm_codegen,
    publish_stored_runtime_wasm_generation,
    runtime_wasm_store,
    stored_runtime_wasm_generation,
)
from molt.cli.runtime_wasm_validation import (
    _is_valid_shared_runtime_wasm_artifact,
    _require_runtime_wasm_validator,
    _shared_runtime_wasm_validation_error,
    RuntimeWasmAdmissionReport,
    runtime_wasm_generation_admission,
)

_SHARED_RUNTIME_NAME = "molt_runtime.wasm"
_RELOC_RUNTIME_NAME = "molt_runtime_reloc.wasm"


@dataclass(frozen=True, slots=True)
class _CombinedRuntimeWasmBuild:
    runtime_state: _RuntimeArtifactState
    shared_spec: _RuntimeWasmBuildSpec | None
    reloc_spec: _RuntimeWasmBuildSpec
    json_output: bool
    cargo_timeout: float | None
    project_root: Path
    simd_enabled: bool
    freestanding: bool

    @property
    def spec(self) -> _RuntimeWasmBuildSpec:
        return self.shared_spec or self.reloc_spec

    @property
    def build_state_root(self) -> Path:
        return _build_state_root(self.project_root)

    def fail(
        self,
        stage: str,
        summary: str,
        *,
        build: subprocess.CompletedProcess[str] | None = None,
        command: tuple[str, ...] = (),
        timed_out: bool = False,
        timeout_error: subprocess.TimeoutExpired | None = None,
    ) -> None:
        record_runtime_wasm_failure(
            self.runtime_state,
            project_root=self.project_root,
            stage=stage,
            summary=summary,
            command=command,
            stdout=_text_output(timeout_error.stdout)
            if timeout_error is not None
            else ""
            if build is None
            else build.stdout,
            stderr=_text_output(timeout_error.stderr)
            if timeout_error is not None
            else ""
            if build is None
            else build.stderr,
            returncode=None if build is None else build.returncode,
            timed_out=timed_out or timeout_error is not None,
            details={"cargo_execution": cargo_execution_evidence(build)}
            if build is not None
            else None,
        )

    def admitted_target_pair_inputs(self) -> CargoEnvironmentInputs | None:
        if self.reloc_spec.staticlib_fingerprint is None:
            return None
        plan = self.spec.cargo_plan
        if plan is None:
            return None
        reloc = _current_runtime_target_artifact(
            _wasm_runtime_staticlib_candidates(
                self.spec.target_root, self.spec.profile_dir
            ),
            build_state_root=self.build_state_root,
            cargo_profile=self.spec.cargo_profile,
            target_label="wasm32-wasip1",
            fingerprint=self.reloc_spec.staticlib_fingerprint,
            environment=plan.environment,
        )
        shared_spec = self.shared_spec
        if reloc is None or shared_spec is None:
            return None if reloc is None else reloc[2]
        if shared_spec.fingerprint is None:
            return None
        shared = _current_runtime_target_artifact(
            _wasm_runtime_wasm_candidates(
                shared_spec.target_root, shared_spec.profile_dir
            ),
            build_state_root=self.build_state_root,
            cargo_profile=shared_spec.cargo_profile,
            target_label="wasm32-wasip1",
            fingerprint=shared_spec.fingerprint,
            environment=plan.environment,
        )
        if (
            shared is not None
            and shared[2] == reloc[2]
            and _is_valid_shared_runtime_wasm_artifact(shared[0])
        ):
            return shared[2]
        return None


def _combined_runtime_wasm_command(
    ctx: _CombinedRuntimeWasmBuild,
) -> tuple[dict[str, str], list[str]]:
    plan = ctx.spec.cargo_plan
    if plan is None:
        raise ValueError("runtime WASM execution requires its resolved Cargo plan")
    return dict(plan.environment), list(plan.command)


def _publish_combined_runtime_wasm_target(
    ctx: _CombinedRuntimeWasmBuild,
    build: subprocess.CompletedProcess[str],
    reported_artifact: Path,
) -> CargoEnvironmentInputs | None:
    artifacts = _reported_runtime_artifacts_from_cargo_stdout(
        build.stdout, target_root=ctx.spec.target_root
    )
    staticlib = artifacts.get(RuntimeCrateType.STATICLIB)
    if staticlib is None or not staticlib.exists():
        return ctx.fail(
            "combined-artifact-selection",
            "Runtime wasm combined build succeeded but Cargo did not report "
            "both runtime crate-type artifacts (expected cdylib and staticlib).",
            build=build,
        )
    targets = [(staticlib, ctx.reloc_spec.staticlib_fingerprint)]
    if ctx.shared_spec is not None:
        cdylib = artifacts.get(RuntimeCrateType.CDYLIB, reported_artifact)
        if not cdylib.exists():
            return ctx.fail(
                "combined-artifact-selection",
                "Runtime wasm combined build succeeded but Cargo did not report "
                "both runtime crate-type artifacts (expected cdylib and staticlib).",
                build=build,
            )
        validation_error = _shared_runtime_wasm_validation_error(cdylib)
        if validation_error is not None:
            return ctx.fail(
                "combined-cdylib-validation",
                "Runtime wasm combined build produced an invalid cdylib artifact: "
                + validation_error,
                build=build,
            )
        targets.insert(0, (cdylib, ctx.shared_spec.fingerprint))
    elif RuntimeCrateType.CDYLIB in artifacts:
        return ctx.fail(
            "combined-artifact-selection",
            "Runtime WASM linked producer reported an unrequested cdylib.",
            build=build,
        )
    try:
        plan = ctx.spec.cargo_plan
        if plan is None:
            raise ValueError("runtime WASM target publication lost its Cargo plan")
        reads = cargo_build_reads(
            build.stdout,
            workspace_root=ctx.project_root,
            dep_info_base=ctx.project_root,
            product_roots=tuple(
                Path(plan.environment[name])
                for name in ("CARGO_TARGET_DIR", "CARGO_BUILD_BUILD_DIR")
                if plan.environment.get(name)
            ),
            toolchain_roots=_wasm_cargo_toolchain_input_paths(plan),
            environment=plan.environment,
        )
        uncovered = uncovered_build_inputs(
            reads,
            ctx.project_root,
            runtime_source_paths(ctx.project_root, ctx.spec.fingerprint_features),
            identity="WASM runtime",
        )
        if uncovered is not None:
            raise ValueError(uncovered)
        plan.verify()
        for artifact, fingerprint in targets:
            fingerprint_path = _runtime_target_fingerprint_path(
                ctx.build_state_root,
                artifact,
                cargo_profile=ctx.spec.cargo_profile,
                target_label="wasm32-wasip1",
            )
            fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
            assert fingerprint is not None
            _write_runtime_fingerprint(
                fingerprint_path,
                fingerprint,
                artifact=artifact,
                cargo_environment=reads.environment,
            )
    except (OSError, ValueError) as exc:
        # StaticArchiveIdentityError (a ValueError) reports an archive that
        # changed while its identity was read; it is a failed stage, not a crash.
        return ctx.fail(
            "combined-target-fingerprint-publication",
            f"Runtime wasm combined build failed to record target fingerprints: {exc}",
            build=build,
        )
    return reads.environment


def _prepopulate_combined_runtime_wasm_target(
    *,
    runtime_state: _RuntimeArtifactState,
    shared_spec: _RuntimeWasmBuildSpec | None,
    reloc_spec: _RuntimeWasmBuildSpec,
    json_output: bool,
    cargo_timeout: float | None,
    project_root: Path,
    simd_enabled: bool,
    freestanding: bool,
) -> CargoEnvironmentInputs | None:
    """Populate the exact shared+reloc target pair with one Cargo transaction."""
    if (
        _require_runtime_wasm_validator(runtime_state, project_root=project_root)
        is None
    ):
        return None
    if (
        (shared_spec is not None and shared_spec.fingerprint is None)
        or reloc_spec.fingerprint is None
        or reloc_spec.staticlib_fingerprint is None
    ):
        record_runtime_wasm_failure(
            runtime_state,
            project_root=project_root,
            stage="combined-build-spec",
            summary="Runtime wasm combined build specification is incomplete.",
        )
        return None
    ctx = _CombinedRuntimeWasmBuild(
        runtime_state,
        shared_spec,
        reloc_spec,
        json_output,
        cargo_timeout,
        project_root,
        simd_enabled,
        freestanding,
    )
    plan = ctx.spec.cargo_plan
    if plan is None:
        return ctx.fail(
            "combined-cargo-plan", "Runtime WASM execution has no resolved Cargo plan."
        )
    try:
        plan.verify()
    except (OSError, ValueError) as exc:
        return ctx.fail("combined-cargo-admission", str(exc), command=plan.command)
    admitted = ctx.admitted_target_pair_inputs()
    if admitted is not None:
        return admitted
    env, cmd = _combined_runtime_wasm_command(ctx)
    if not json_output:
        _progress.notice("WASM runtime artifacts need one combined source build")
    started = time.perf_counter()
    try:
        build, reported_artifact = _run_runtime_wasm_cargo_build(
            cargo_plan=plan,
            cargo_timeout=cargo_timeout,
            profile_dir=ctx.spec.profile_dir,
            target_root_override=ctx.spec.target_root,
            json_output=json_output,
            artifact_kind=RuntimeCrateType.CDYLIB
            if shared_spec is not None
            else RuntimeCrateType.STATICLIB,
        )
    except CargoPlanExecutionError as exc:
        return ctx.fail(
            "combined-cargo-identity-stability",
            str(exc),
            build=exc.cargo_result,
            command=tuple(cmd),
        )
    except (OSError, ValueError) as exc:
        return ctx.fail("combined-cargo-admission", str(exc), command=tuple(cmd))
    except subprocess.TimeoutExpired as exc:
        return ctx.fail(
            "combined-cargo",
            "Runtime wasm combined build timed out.",
            command=tuple(cmd),
            timed_out=True,
            timeout_error=exc,
        )
    if build.returncode != 0:
        # A guard timeout raised TimeoutExpired above; Cargo's own exit status
        # is never read as one.
        return ctx.fail(
            "combined-cargo",
            "Runtime wasm combined build failed",
            build=build,
            command=tuple(cmd),
        )
    _record_runtime_wasm_build_phase(
        "cargo_compile",
        time.perf_counter() - started,
        kind="combined" if shared_spec is not None else "reloc",
        mode="build",
        detail=ctx.spec.build_policy.diagnostic_detail,
    )
    return _publish_combined_runtime_wasm_target(ctx, build, reported_artifact)


def _select_runtime_wasm_generation(
    runtime_state: _RuntimeArtifactState, generation: RuntimeWasmRelocGeneration
) -> None:
    """Record the admitted runtime used by code generation and final link."""
    runtime_state.runtime_wasm_pair = generation


@dataclass(frozen=True, slots=True)
class _RuntimeWasmPairIdentity:
    toolchain: RuntimeToolchainContentManifest
    shared: RuntimeBuildIdentity | None
    reloc: RuntimeBuildIdentity
    input_observation: RuntimeBuildInputObservation | None = field(
        default=None, compare=False
    )
    shared_spec: _RuntimeWasmBuildSpec | None = field(default=None, compare=False)
    reloc_spec: _RuntimeWasmBuildSpec | None = field(default=None, compare=False)


def _bind_exact_family_fingerprints(
    ctx: _RuntimeWasmPairBuild,
    identity: _RuntimeWasmPairIdentity,
) -> None:
    """Use the generation authority for Cargo-target admission as well."""

    compile_fingerprint = runtime_build_fingerprint(identity.reloc, scope="compile")
    if ctx.shared_spec is not None:
        assert identity.shared is not None
        ctx.shared_spec = ctx.shared_spec._replace(
            fingerprint=runtime_build_fingerprint(
                identity.shared, scope="member-output"
            ),
            staticlib_fingerprint=compile_fingerprint,
        )
    ctx.reloc_spec = ctx.reloc_spec._replace(
        fingerprint=runtime_build_fingerprint(identity.reloc, scope="member-output"),
        staticlib_fingerprint=compile_fingerprint,
    )


@dataclass(slots=True)
class _RuntimeWasmPairBuild:
    runtime_state: _RuntimeArtifactState
    json_output: bool
    cargo_profile: str
    cargo_timeout: float | None
    project_root: Path
    simd_enabled: bool
    freestanding: bool
    stdlib_profile: str | None
    resolved_modules: set[str] | frozenset[str] | None
    required_link_features: frozenset[str]
    required_exports: set[str] | frozenset[str] | None
    shared_spec: _RuntimeWasmBuildSpec | None
    reloc_spec: _RuntimeWasmBuildSpec
    store: ProductStore
    pre_identity: _RuntimeWasmPairIdentity | None
    codegen_use: RuntimeWasmCodegenUse = RuntimeWasmCodegenUse.DEPLOYED_RUNTIME
    staging_root: Path | None = None
    staging_shared: Path | None = None
    staging_reloc: Path | None = None

    cargo_environment: CargoEnvironmentInputs | None = None
    accepted_generation: SourceWasmGeneration | None = None
    admission_report: RuntimeWasmAdmissionReport | None = None

    def failure_details(self) -> dict[str, object]:
        identity = self.pre_identity
        return {
            "family_digest": (
                None if identity is None else identity.reloc.family_digest
            ),
            "stdlib_profile": self.stdlib_profile,
            "required_link_features": sorted(self.required_link_features),
            "required_exports": (
                None if self.required_exports is None else sorted(self.required_exports)
            ),
            "store_entry": (
                None
                if identity is None
                else str(self.store.entry(identity.reloc.family_digest))
            ),
            "staging_root": (
                None if self.staging_root is None else str(self.staging_root)
            ),
            "staging_shared": (
                None if self.staging_shared is None else str(self.staging_shared)
            ),
            "staging_shared_exists": bool(
                self.staging_shared is not None and self.staging_shared.is_file()
            ),
            "staging_reloc": (
                None if self.staging_reloc is None else str(self.staging_reloc)
            ),
            "staging_reloc_exists": bool(
                self.staging_reloc is not None and self.staging_reloc.is_file()
            ),
        }

    def fail(self, stage: str, summary: str) -> bool:
        return record_runtime_wasm_failure(
            self.runtime_state,
            project_root=self.project_root,
            stage=stage,
            summary=summary,
            details=self.failure_details(),
        )

    def accept_generation(self, generation: RuntimeWasmRelocGeneration) -> bool:
        """Admit ``generation`` for this request: its identity, environment and exports."""
        self.accepted_generation = None
        self.admission_report = None
        identity = self.pre_identity
        plan = self.reloc_spec.cargo_plan
        if (
            identity is None
            or plan is None
            or not isinstance(
                generation,
                (SourceRuntimeWasmGeneration, SourceLinkedRuntimeWasmGeneration),
            )
            or (
                generation.shared_identity
                if isinstance(generation, SourceRuntimeWasmGeneration)
                else None
            )
            != identity.shared
            or generation.reloc_identity != identity.reloc
        ):
            return False
        try:
            generation.cargo_environment.verify(plan.environment)
        except ValueError:
            return False
        self.admission_report = runtime_wasm_generation_admission(
            generation, self.required_exports, use=self.codegen_use
        )
        if not self.admission_report.accepted:
            return False
        _select_runtime_wasm_generation(self.runtime_state, generation)
        self.accepted_generation = generation
        self.runtime_state.runtime_wasm_build_inputs = identity
        return True

    def generation_rejection_details(self) -> dict[str, object]:
        if self.admission_report is None:
            return {
                "generation": "manifest, member content, or identity validation failed"
            }
        return self.admission_report.details()

    def provision_staging(self) -> None:
        identity = self.pre_identity
        if identity is None:
            raise ValueError("runtime WASM staging requires an exact pair identity")
        root = (
            _build_state_root(self.project_root)
            / "runtime_wasm_staging"
            / identity.reloc.family_digest
            / uuid.uuid4().hex
        )
        root.mkdir(parents=True, exist_ok=False)
        self.staging_root = root
        self.staging_shared = (
            root / _SHARED_RUNTIME_NAME if self.shared_spec is not None else None
        )
        self.staging_reloc = root / _RELOC_RUNTIME_NAME

    def cleanup_staging(self) -> None:
        for staging in (self.staging_shared, self.staging_reloc):
            if staging is not None:
                with contextlib.suppress(OSError):
                    staging.unlink(missing_ok=True)
        if self.staging_root is not None:
            with contextlib.suppress(OSError):
                self.staging_root.rmdir()

    def staging_member(self, *, reloc: bool) -> Path:
        member = self.staging_reloc if reloc else self.staging_shared
        if member is None:
            raise ValueError("runtime WASM member staging is not provisioned")
        return member

    def ensure_member(self, *, reloc: bool) -> bool:
        try:
            if self.cargo_environment is None:
                raise ValueError("runtime WASM member lacks its compile observation")
            spec = self.reloc_spec if reloc else self.shared_spec
            if spec is None:
                raise ValueError("runtime WASM member was not selected")
            return _materialize_runtime_wasm_member_from_target(
                self.staging_member(reloc=reloc),
                reloc=reloc,
                json_output=self.json_output,
                cargo_timeout=self.cargo_timeout,
                project_root=self.project_root,
                resolved_modules=self.resolved_modules,
                required_exports=self.required_exports,
                spec=spec,
                cargo_environment=self.cargo_environment,
            )
        except RuntimeWasmLinkError as exc:
            process = exc.process
            return record_runtime_wasm_failure(
                self.runtime_state,
                project_root=self.project_root,
                stage="reloc-link",
                summary=str(exc),
                command=exc.command,
                stdout=exc.stdout,
                stderr=exc.stderr,
                returncode=None if process is None else process.returncode,
                timed_out=exc.timed_out,
                details=self.failure_details(),
            )
        except (OSError, ValueError, subprocess.SubprocessError) as exc:
            return self.fail("member-publication", str(exc))


def resolve_runtime_wasm_pair_identity(
    project_root: Path,
    shared_spec: _RuntimeWasmBuildSpec | None,
    reloc_spec: _RuntimeWasmBuildSpec,
    *,
    mode: Literal["pre_build", "post_build"] = "pre_build",
    retained_inputs: _RuntimeWasmPairIdentity | None = None,
) -> _RuntimeWasmPairIdentity:
    """Capture a checkout's resolved inputs without building either member."""
    selected_shared = (
        None
        if shared_spec is None
        else shared_spec._replace(fingerprint=None, staticlib_fingerprint=None)
    )
    selected_reloc = reloc_spec._replace(fingerprint=None, staticlib_fingerprint=None)
    if (
        mode == "pre_build"
        and retained_inputs is not None
        and retained_inputs.shared_spec == selected_shared
        and retained_inputs.reloc_spec == selected_reloc
        and retained_inputs.input_observation is not None
    ):
        try:
            if reloc_spec.cargo_plan is None:
                raise ValueError("runtime WASM reuse has no current Cargo plan")
            retained_inputs.input_observation.verify(
                runtime_source_roots(project_root, reloc_spec.fingerprint_features)
            )
            reloc_spec.cargo_plan.verify()
        except (OSError, ValueError):
            pass
        else:
            return retained_inputs
    capture = RuntimeBuildInputCapture()
    shared, reloc = _timed_runtime_identity_phase(
        phase="runtime_family_identity",
        mode=mode,
        operation=lambda: _resolved_runtime_wasm_family_identities(
            project_root,
            shared_spec,
            reloc_spec,
            input_observation=capture,
        ),
        identity_tree=_runtime_source_identity_tree,
    )
    return _RuntimeWasmPairIdentity(
        reloc.toolchain_manifest,
        shared,
        reloc,
        capture.finish(),
        selected_shared,
        selected_reloc,
    )


def resolve_runtime_wasm_pair_specs(
    project_root: Path,
    *,
    build_profile: BuildProfile,
    cargo_profile: str,
    simd_enabled: bool,
    freestanding: bool,
    stdlib_profile: str | None,
    resolved_modules: set[str] | frozenset[str] | None,
    required_link_features: frozenset[str],
    required_exports: set[str] | frozenset[str] | None,
    full_export_surface: bool = False,
    codegen_use: RuntimeWasmCodegenUse = RuntimeWasmCodegenUse.DEPLOYED_RUNTIME,
) -> tuple[_RuntimeWasmBuildSpec | None, _RuntimeWasmBuildSpec]:
    """Resolve the required runtime members through one Cargo plan."""

    def spec(*, reloc: bool) -> _RuntimeWasmBuildSpec:
        return _compute_runtime_wasm_build_spec(
            project_root,
            _RELOC_RUNTIME_NAME if reloc else _SHARED_RUNTIME_NAME,
            reloc=reloc,
            build_profile=build_profile,
            cargo_profile=cargo_profile,
            simd_enabled=simd_enabled,
            freestanding=freestanding,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
            required_link_features=required_link_features,
            required_exports=required_exports,
            full_export_surface=full_export_surface,
        )

    return _resolve_runtime_wasm_cargo_specs(
        project_root,
        None if codegen_use is RuntimeWasmCodegenUse.LINKED else spec(reloc=False),
        spec(reloc=True),
        simd_enabled=simd_enabled,
        freestanding=freestanding,
    )


def _prepare_runtime_wasm_pair_build(
    runtime_state: _RuntimeArtifactState,
    *,
    json_output: bool,
    build_profile: BuildProfile,
    cargo_profile: str,
    cargo_timeout: float | None,
    project_root: Path,
    simd_enabled: bool,
    freestanding: bool,
    stdlib_profile: str | None,
    resolved_modules: set[str] | frozenset[str] | None,
    required_link_features: frozenset[str],
    required_exports: set[str] | frozenset[str] | None,
    full_export_surface: bool = False,
    codegen_use: RuntimeWasmCodegenUse = RuntimeWasmCodegenUse.DEPLOYED_RUNTIME,
) -> _RuntimeWasmPairBuild | None:
    def fail(summary: str) -> None:
        record_runtime_wasm_failure(
            runtime_state,
            project_root=project_root,
            stage="identity-provisioning",
            summary=summary,
        )

    try:
        shared_spec, reloc_spec = resolve_runtime_wasm_pair_specs(
            project_root,
            build_profile=build_profile,
            cargo_profile=cargo_profile,
            simd_enabled=simd_enabled,
            freestanding=freestanding,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
            required_link_features=required_link_features,
            required_exports=required_exports,
            full_export_surface=full_export_surface,
            codegen_use=codegen_use,
        )
        identity = resolve_runtime_wasm_pair_identity(
            project_root,
            shared_spec,
            reloc_spec,
            mode="pre_build",
            retained_inputs=(
                runtime_state.runtime_wasm_build_inputs
                if runtime_state.runtime_wasm_codegen_binding is not None
                else None
            ),
        )
        store = runtime_wasm_store(project_root)
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        fail(f"Runtime WASM identity provisioning failed: {exc}")
        return None
    ctx = _RuntimeWasmPairBuild(
        runtime_state,
        json_output,
        cargo_profile,
        cargo_timeout,
        project_root,
        simd_enabled,
        freestanding,
        stdlib_profile,
        resolved_modules,
        required_link_features,
        required_exports,
        shared_spec,
        reloc_spec,
        store,
        identity,
        codegen_use=codegen_use,
    )
    _bind_exact_family_fingerprints(ctx, identity)
    return ctx


def _build_runtime_wasm_pair_members(ctx: _RuntimeWasmPairBuild) -> bool:
    """Run the combined Cargo transaction and finalize both staged members."""
    try:
        ctx.provision_staging()
    except (OSError, ValueError) as exc:
        return ctx.fail("pair-staging", f"Runtime WASM pair staging failed: {exc}")
    ctx.cargo_environment = _prepopulate_combined_runtime_wasm_target(
        runtime_state=ctx.runtime_state,
        shared_spec=ctx.shared_spec,
        reloc_spec=ctx.reloc_spec,
        json_output=ctx.json_output,
        cargo_timeout=ctx.cargo_timeout,
        project_root=ctx.project_root,
        simd_enabled=ctx.simd_enabled,
        freestanding=ctx.freestanding,
    )
    if ctx.cargo_environment is None:
        return False
    for reloc, stage, label in (
        (False, "shared-member-publication", "shared"),
        (True, "reloc-member-publication", "relocatable"),
    ):
        if not reloc and ctx.shared_spec is None:
            continue
        if not ctx.ensure_member(reloc=reloc):
            if ctx.runtime_state.runtime_wasm_build_failure is None:
                ctx.fail(
                    stage,
                    f"Runtime WASM combined target could not publish the {label} member.",
                )
            return False
    return True


def _publish_runtime_wasm_pair(ctx: _RuntimeWasmPairBuild) -> bool:
    try:
        plan = ctx.reloc_spec.cargo_plan
        if plan is None:
            raise ValueError("runtime WASM build lost its resolved Cargo plan")
        post_identity = resolve_runtime_wasm_pair_identity(
            ctx.project_root,
            None if ctx.shared_spec is None else ctx.shared_spec.with_cargo_plan(plan),
            ctx.reloc_spec.with_cargo_plan(plan),
            mode="post_build",
        )
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        return ctx.fail(
            "post-build-identity",
            f"Runtime WASM post-build identity failed: {exc}",
        )
    if ctx.pre_identity is None or post_identity != ctx.pre_identity:
        return ctx.fail(
            "identity-stability",
            "Runtime build identity changed during Cargo; refusing publication.",
        )
    try:
        if ctx.cargo_environment is None:
            raise ValueError(
                "runtime WASM pair lacks its Cargo environment observation"
            )
        ctx.cargo_environment.verify(plan.environment)
        published = publish_stored_runtime_wasm_generation(
            ctx.store,
            shared_identity=post_identity.shared,
            reloc_identity=post_identity.reloc,
            cargo_environment=ctx.cargo_environment,
            source_shared=None
            if ctx.shared_spec is None
            else ctx.staging_member(reloc=False),
            source_reloc=ctx.staging_member(reloc=True),
        )
    except (OSError, ValueError) as exc:
        return ctx.fail(
            "generation-publication",
            f"Runtime WASM pair publication failed: {exc}",
        )
    if ctx.accept_generation(published):
        return True
    return ctx.fail(
        "generation-acceptance",
        "Published Runtime WASM generation failed immutable acceptance: "
        + json.dumps(ctx.generation_rejection_details(), sort_keys=True),
    )


def _build_and_publish_runtime_wasm_pair(ctx: _RuntimeWasmPairBuild) -> bool:
    """Build the pair once under the family's store lock, then publish it.

    The checkout's pair lock is Cargo target custody: two builds of other
    families in this checkout share one Cargo target, and one must not
    rewrite the archives while the other fingerprints them (HF-F227).
    """
    if reason := source_build_disabled_reason(
        "Runtime WASM pair"
        if ctx.shared_spec is not None
        else "Relocatable WASM runtime"
    ):
        return ctx.fail("rebuild-policy", reason)
    with _build_lock(
        ctx.project_root, f"runtime.{ctx.cargo_profile}.wasm32-wasip1.pair"
    ):
        return _build_runtime_wasm_pair_members(ctx) and _publish_runtime_wasm_pair(ctx)


def _ensure_stored_runtime_wasm_pair(ctx: _RuntimeWasmPairBuild) -> bool:
    """Admit this request's stored pair, or build and publish it once.

    A hit reads the store without a lock and runs no Cargo. A miss holds the
    family's store lock, so concurrent builds with the same inputs, in this
    checkout or any other, compile once and the waiters admit the result.
    """
    identity = ctx.pre_identity
    plan = ctx.reloc_spec.cargo_plan
    if identity is None or plan is None:
        return ctx.fail("pair-preparation", "Runtime WASM pair has no exact identity.")

    family = identity.reloc.family_digest
    started = time.perf_counter()

    def record(mode: str) -> None:
        _record_runtime_wasm_build_phase(
            "product_store",
            time.perf_counter() - started,
            kind="family",
            mode=mode,
            detail=f"entry={family}",
        )

    def read() -> bool | None:
        generation = stored_runtime_wasm_generation(
            ctx.store,
            shared_identity=identity.shared,
            reloc_identity=identity.reloc,
            environment=plan.environment,
        )
        if generation is None:
            return None
        record("hit")
        if ctx.accept_generation(generation):
            return True
        return ctx.fail(
            "store-admission",
            "The stored runtime WASM pair failed admission: "
            + json.dumps(ctx.generation_rejection_details(), sort_keys=True),
        )

    try:

        def build() -> bool:
            built = _build_and_publish_runtime_wasm_pair(ctx)
            record("miss")
            return built

        return ctx.store.ensure(family, read, build)
    except (ProductStoreError, OSError) as exc:
        return ctx.fail("product-store", f"Runtime WASM product store: {exc}")
    except BuildLockAcquisitionError as exc:
        return ctx.fail("pair-build-lock", f"Runtime WASM pair build lock: {exc}")


def _ensure_installed_runtime_wasm(
    runtime_state: _RuntimeArtifactState,
    cell: InstalledRuntimeCell,
    *,
    project_root: Path,
    required_link_features: frozenset[str],
    required_exports: set[str] | frozenset[str] | None,
    planned_exports: set[str] | frozenset[str] | None,
    bind_for_codegen: bool,
    codegen_use: RuntimeWasmCodegenUse = RuntimeWasmCodegenUse.DEPLOYED_RUNTIME,
) -> bool:
    """Admit the shipped pair once; an installed CLI never plans or runs Cargo.

    After app layout is bound, the operation reuses that exact retained pair.
    Selection and required features are re-derived for the request and the
    members' stable-file fences are checked; the cell is never admitted again
    beneath compiled code.
    """
    details = {
        "installed_runtime_cell": cell.id,
        "required_link_features": sorted(required_link_features),
    }
    binding = runtime_state.runtime_wasm_codegen_binding

    def fail(stage: str, summary: str) -> bool:
        if binding is not None:
            # A bound pair that failed reuse must not authorize a later link.
            runtime_state.runtime_wasm_codegen_binding = None
        return record_runtime_wasm_failure(
            runtime_state,
            project_root=project_root,
            stage=stage,
            summary=summary,
            details=details,
        )

    if binding is None:
        try:
            generation = admit_installed_wasm_runtime(
                cell, required_link_features=required_link_features
            )
        except ValueError as exc:
            return fail("installed-runtime-admission", str(exc))
    else:
        try:
            binding.verify()
            if not isinstance(binding.generation, RuntimeWasmGeneration):
                raise ValueError("installed WASM runtime requires its signed pair")
            reuse_installed_wasm_generation(
                cell,
                binding.generation,
                required_link_features=required_link_features,
            )
        except ValueError as exc:
            return fail(
                "codegen-identity-stability",
                "Installed runtime WASM cell changed after app layout was bound; "
                f"refusing runtime reselection beneath compiled code: {exc}",
            )
        generation = binding.generation
    exports = required_exports if binding is not None else planned_exports
    report = runtime_wasm_generation_admission(generation, exports, use=codegen_use)
    if not report.accepted:
        return fail(
            "installed-runtime-exports",
            f"Installed runtime WASM cell {cell.id} failed runtime admission: "
            + json.dumps(report.details(), sort_keys=True),
        )
    _select_runtime_wasm_generation(runtime_state, generation)
    if bind_for_codegen and binding is None:
        try:
            binding = bind_runtime_wasm_codegen(
                generation, planned_exports, use=codegen_use
            )
        except (OSError, ValueError) as exc:
            return fail(
                "codegen-binding", f"Runtime WASM codegen binding failed: {exc}"
            )
        runtime_state.runtime_wasm_codegen_binding = binding
    return True


def _ensure_runtime_wasm_both(
    runtime_state: _RuntimeArtifactState,
    *,
    json_output: bool,
    build_profile: BuildProfile,
    cargo_profile: str,
    cargo_timeout: float | None,
    project_root: Path,
    simd_enabled: bool,
    freestanding: bool,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: set[str] | frozenset[str] | None = None,
    required_link_features: frozenset[str] = frozenset(),
    required_exports: set[str] | frozenset[str] | None = None,
    bind_for_codegen: bool = False,
    codegen_use: RuntimeWasmCodegenUse | None = None,
) -> bool:
    runtime_state.runtime_wasm_build_failure = None
    binding = runtime_state.runtime_wasm_codegen_binding
    if codegen_use is None:
        codegen_use = (
            binding.use
            if binding is not None
            else RuntimeWasmCodegenUse.DEPLOYED_RUNTIME
        )
    elif binding is not None and codegen_use is not binding.use:
        return record_runtime_wasm_failure(
            runtime_state,
            project_root=project_root,
            stage="codegen-identity-stability",
            summary="Runtime WASM output mode changed after code generation was bound.",
        )
    # Validator availability is a prerequisite, not an artifact cache miss.
    # Refuse before source/toolchain identity planning or any Cargo transaction.
    if (
        _require_runtime_wasm_validator(runtime_state, project_root=project_root)
        is None
    ):
        return False
    # Once app layout is emitted, imports are admission obligations, not a new
    # runtime build plan. Recapture current source/toolchain identity using the
    # original plan; never rebuild or switch physical members beneath the app.
    planned_exports = (
        binding.required_exports if binding is not None else required_exports
    )
    try:
        installed = select_installed_wasm_runtime(
            project_root,
            cargo_profile=cargo_profile,
            stdlib_profile=stdlib_profile,
            simd_enabled=simd_enabled,
            freestanding=freestanding,
        )
    except ValueError as exc:
        return record_runtime_wasm_failure(
            runtime_state,
            project_root=project_root,
            stage="installed-runtime-selection",
            summary=str(exc),
        )
    if installed is not None:
        return _ensure_installed_runtime_wasm(
            runtime_state,
            installed,
            project_root=project_root,
            required_link_features=required_link_features,
            required_exports=required_exports,
            planned_exports=planned_exports,
            bind_for_codegen=bind_for_codegen,
            codegen_use=codegen_use,
        )
    ctx = _prepare_runtime_wasm_pair_build(
        runtime_state,
        json_output=json_output,
        build_profile=build_profile,
        cargo_profile=cargo_profile,
        cargo_timeout=cargo_timeout,
        project_root=project_root,
        simd_enabled=simd_enabled,
        freestanding=freestanding,
        stdlib_profile=stdlib_profile,
        resolved_modules=resolved_modules,
        required_link_features=required_link_features,
        required_exports=planned_exports,
        full_export_surface=bind_for_codegen or binding is not None,
        codegen_use=codegen_use,
    )
    if ctx is None:
        return False
    try:
        if binding is not None:
            generation = binding.generation
            if not isinstance(
                generation,
                (SourceRuntimeWasmGeneration, SourceLinkedRuntimeWasmGeneration),
            ):
                return ctx.fail(
                    "codegen-generation-admission",
                    "Installed runtime product cannot authorize source-build reuse.",
                )
            if ctx.pre_identity is None or (
                ctx.pre_identity.shared
                != (
                    generation.shared_identity
                    if isinstance(generation, SourceRuntimeWasmGeneration)
                    else None
                )
                or ctx.pre_identity.reloc != generation.reloc_identity
            ):
                return ctx.fail(
                    "codegen-identity-stability",
                    "Runtime WASM build inputs changed after app layout was bound; "
                    "refusing runtime reselection beneath compiled code.",
                )
            ctx.required_exports = required_exports
            try:
                binding.verify()
            except (OSError, ValueError) as exc:
                return ctx.fail(
                    "codegen-identity-stability",
                    f"The bound runtime WASM generation changed: {exc}",
                )
            if ctx.accept_generation(generation):
                return True
            return ctx.fail(
                "codegen-generation-admission",
                "The runtime WASM pair bound before app code generation no longer "
                "satisfies its emitted import contract: "
                + json.dumps(ctx.generation_rejection_details(), sort_keys=True),
            )
        if not _ensure_stored_runtime_wasm_pair(ctx):
            return False
        if bind_for_codegen:
            assert ctx.pre_identity is not None
            generation = ctx.accepted_generation
            if generation is None:
                return ctx.fail(
                    "codegen-binding",
                    "Runtime WASM pair lost admission before code generation.",
                )
            try:
                binding = bind_runtime_wasm_codegen(
                    generation, planned_exports, use=codegen_use
                )
            except (OSError, ValueError) as exc:
                return ctx.fail(
                    "codegen-binding", f"Runtime WASM codegen binding failed: {exc}"
                )
            runtime_state.runtime_wasm_codegen_binding = binding
        return True
    finally:
        ctx.cleanup_staging()
