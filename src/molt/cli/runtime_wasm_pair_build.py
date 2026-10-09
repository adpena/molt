from __future__ import annotations

from molt.cli import progress as _progress
from molt.cli.runtime_build_python import build_python_scope

import contextlib
import json
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass
from enum import Enum, auto
from pathlib import Path
from typing import Literal

from molt.cargo_execution_policy import source_build_disabled_reason
from molt.cli.artifact_state import (
    _build_state_root,
    _runtime_target_fingerprint_path,
)
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
from molt.cli.runtime_wasm_build import _materialize_runtime_wasm_member_from_target
from molt.cli.runtime_wasm_build_spec import (
    _compute_runtime_wasm_build_spec,
    _resolve_runtime_wasm_cargo_specs,
    _resolved_runtime_wasm_family_identities,
    _runtime_source_identity_tree,
    _runtime_wasm_toolchain_manifest_path,
    _RuntimeWasmBuildSpec,
    _timed_runtime_identity_phase,
)
from molt.cli.runtime_wasm_build_support import (
    RuntimeWasmLinkError,
    _current_runtime_target_artifact,
    _reported_runtime_artifacts_from_cargo_stdout,
    _run_runtime_wasm_cargo_build,
    _wasm_runtime_staticlib_candidates,
    _wasm_runtime_wasm_candidates,
)
from molt.cli.runtime_wasm_build_timings import (
    _record_runtime_wasm_build_phase,
)
from molt.cli.runtime_wasm_cache import (
    hydrate_runtime_wasm_pair_from_shared_cache,
    publish_runtime_wasm_pair_to_shared_cache,
)
from molt.cli.runtime_wasm_failure import record_runtime_wasm_failure
from molt.cli.runtime_wasm_generation import (
    RuntimeWasmExpectedPair,
    RuntimeWasmGeneration,
    bind_runtime_wasm_codegen,
    publish_runtime_wasm_generation,
    read_runtime_wasm_generation,
    runtime_wasm_generation_path,
)
from molt.cli.runtime_wasm_validation import (
    _is_valid_shared_runtime_wasm_artifact,
    RuntimeWasmAdmissionReport,
    runtime_wasm_generation_admission,
)
from molt.wasm_artifact import (
    inspect_wasm_binary as _inspect_wasm_binary,
)


def _warn_runtime_wasm_cache_publish_failure(
    failure: str | None,
    *,
    json_output: bool,
) -> None:
    if failure is None or json_output:
        return
    print(
        f"Warning: runtime wasm shared cache publish failed: {failure}",
        file=sys.stderr,
    )


@dataclass(frozen=True, slots=True)
class _CombinedRuntimeWasmBuild:
    runtime_state: _RuntimeArtifactState
    shared_spec: _RuntimeWasmBuildSpec
    reloc_spec: _RuntimeWasmBuildSpec
    json_output: bool
    cargo_timeout: float | None
    project_root: Path
    simd_enabled: bool
    freestanding: bool

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
    ) -> bool:
        return record_runtime_wasm_failure(
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

    def target_pair_is_current(self) -> bool:
        if (
            self.shared_spec.fingerprint is None
            or self.reloc_spec.staticlib_fingerprint is None
        ):
            return False
        shared = _current_runtime_target_artifact(
            _wasm_runtime_wasm_candidates(
                self.shared_spec.target_root, self.shared_spec.profile_dir
            ),
            build_state_root=self.build_state_root,
            cargo_profile=self.shared_spec.cargo_profile,
            target_label="wasm32-wasip1",
            fingerprint=self.shared_spec.fingerprint,
        )
        reloc = _current_runtime_target_artifact(
            _wasm_runtime_staticlib_candidates(
                self.shared_spec.target_root, self.shared_spec.profile_dir
            ),
            build_state_root=self.build_state_root,
            cargo_profile=self.shared_spec.cargo_profile,
            target_label="wasm32-wasip1",
            fingerprint=self.reloc_spec.staticlib_fingerprint,
        )
        return (
            shared is not None
            and reloc is not None
            and _is_valid_shared_runtime_wasm_artifact(shared[0])
        )


def _combined_runtime_wasm_command(
    ctx: _CombinedRuntimeWasmBuild,
) -> tuple[dict[str, str], list[str]]:
    plan = ctx.shared_spec.cargo_plan
    if plan is None:
        raise ValueError("runtime WASM execution requires its resolved Cargo plan")
    return dict(plan.environment), list(plan.command)


def _publish_combined_runtime_wasm_target(
    ctx: _CombinedRuntimeWasmBuild,
    build: subprocess.CompletedProcess[str],
    reported_cdylib: Path,
) -> bool:
    artifacts = _reported_runtime_artifacts_from_cargo_stdout(
        build.stdout, target_root=ctx.shared_spec.target_root
    )
    cdylib = artifacts.get(RuntimeCrateType.CDYLIB, reported_cdylib)
    staticlib = artifacts.get(RuntimeCrateType.STATICLIB)
    if not cdylib.exists() or staticlib is None or not staticlib.exists():
        return ctx.fail(
            "combined-artifact-selection",
            "Runtime wasm combined build succeeded but Cargo did not report "
            "both runtime crate-type artifacts (expected cdylib and staticlib).",
            build=build,
        )
    if _inspect_wasm_binary(
        cdylib
    ) != "valid" or not _is_valid_shared_runtime_wasm_artifact(cdylib):
        return ctx.fail(
            "combined-cdylib-validation",
            "Runtime wasm combined build produced an invalid cdylib artifact.",
            build=build,
        )
    try:
        for artifact, fingerprint in (
            (cdylib, ctx.shared_spec.fingerprint),
            (staticlib, ctx.reloc_spec.staticlib_fingerprint),
        ):
            fingerprint_path = _runtime_target_fingerprint_path(
                ctx.build_state_root,
                artifact,
                cargo_profile=ctx.shared_spec.cargo_profile,
                target_label="wasm32-wasip1",
            )
            fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
            assert fingerprint is not None
            _write_runtime_fingerprint(fingerprint_path, fingerprint, artifact=artifact)
    except OSError as exc:
        return ctx.fail(
            "combined-target-fingerprint-publication",
            f"Runtime wasm combined build failed to record target fingerprints: {exc}",
            build=build,
        )
    return True


def _prepopulate_combined_runtime_wasm_target(
    *,
    runtime_state: _RuntimeArtifactState,
    shared_spec: _RuntimeWasmBuildSpec,
    reloc_spec: _RuntimeWasmBuildSpec,
    json_output: bool,
    cargo_timeout: float | None,
    project_root: Path,
    simd_enabled: bool,
    freestanding: bool,
    force_build: bool = False,
) -> bool:
    """Populate the exact shared+reloc target pair with one Cargo transaction."""
    if (
        shared_spec.fingerprint is None
        or reloc_spec.fingerprint is None
        or reloc_spec.staticlib_fingerprint is None
    ):
        return record_runtime_wasm_failure(
            runtime_state,
            project_root=project_root,
            stage="combined-build-spec",
            summary="Runtime wasm combined build specification is incomplete.",
        )
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
    plan = ctx.shared_spec.cargo_plan
    if plan is None:
        return ctx.fail(
            "combined-cargo-plan", "Runtime WASM execution has no resolved Cargo plan."
        )
    try:
        plan.verify()
    except (OSError, ValueError) as exc:
        return ctx.fail("combined-cargo-admission", str(exc), command=plan.command)
    if not force_build and ctx.target_pair_is_current():
        return True
    env, cmd = _combined_runtime_wasm_command(ctx)
    if not json_output:
        _progress.notice("WASM runtime artifacts need one combined source build")
    started = time.perf_counter()
    try:
        build, reported_cdylib = _run_runtime_wasm_cargo_build(
            cargo_plan=plan,
            cargo_timeout=cargo_timeout,
            profile_dir=shared_spec.profile_dir,
            target_root_override=shared_spec.target_root,
            json_output=json_output,
            artifact_kind=RuntimeCrateType.CDYLIB,
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
        kind="combined",
        mode="build",
        detail=(
            "target_dir=stable-incremental (cross-session dep cache)"
            if shared_spec.incremental_enabled
            else "target_dir=session"
        ),
    )
    return _publish_combined_runtime_wasm_target(ctx, build, reported_cdylib)


def _select_runtime_wasm_generation(
    runtime_state: _RuntimeArtifactState,
    *,
    project_root: Path,
    generation: RuntimeWasmGeneration,
) -> bool:
    """Publish the expected-pair receipt final link admits, then select."""
    expected_path = (
        _build_state_root(project_root)
        / "runtime_wasm_generations"
        / f"{generation.shared_identity.family_digest}.expected.json"
    )
    try:
        RuntimeWasmExpectedPair(
            generation.shared_identity, generation.reloc_identity
        ).write(expected_path)
    except (OSError, ValueError):
        return False
    runtime_state.runtime_wasm_generation = generation.manifest
    runtime_state.runtime_wasm_selected = generation.shared
    runtime_state.runtime_reloc_wasm_selected = generation.reloc
    runtime_state.runtime_wasm_expected_identity = expected_path
    return True


@dataclass(frozen=True, slots=True)
class _RuntimeWasmPairIdentity:
    toolchain: RuntimeToolchainContentManifest
    shared: RuntimeBuildIdentity
    reloc: RuntimeBuildIdentity


def _bind_exact_family_fingerprints(
    ctx: _RuntimeWasmPairBuild,
    identity: _RuntimeWasmPairIdentity,
) -> None:
    """Use the generation authority for Cargo-target admission as well."""

    compile_fingerprint = runtime_build_fingerprint(identity.shared, scope="compile")
    ctx.shared_spec = ctx.shared_spec._replace(
        fingerprint=runtime_build_fingerprint(identity.shared, scope="member-output"),
        staticlib_fingerprint=compile_fingerprint,
    )
    ctx.reloc_spec = ctx.reloc_spec._replace(
        fingerprint=runtime_build_fingerprint(identity.reloc, scope="member-output"),
        staticlib_fingerprint=compile_fingerprint,
    )


class _PairBuildOutcome(Enum):
    ACCEPTED = auto()
    BUILT = auto()
    FAILED = auto()


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
    runtime_wasm: Path
    runtime_reloc_wasm: Path
    shared_spec: _RuntimeWasmBuildSpec
    reloc_spec: _RuntimeWasmBuildSpec
    toolchain_manifest_path: Path
    generation_manifest: Path
    pre_identity: _RuntimeWasmPairIdentity | None
    staging_root: Path | None = None
    staging_shared: Path | None = None
    staging_reloc: Path | None = None

    accepted_generation: RuntimeWasmGeneration | None = None
    admission_report: RuntimeWasmAdmissionReport | None = None

    def failure_details(self) -> dict[str, object]:
        identity = self.pre_identity
        return {
            "family_digest": (
                None if identity is None else identity.shared.family_digest
            ),
            "stdlib_profile": self.stdlib_profile,
            "required_link_features": sorted(self.required_link_features),
            "required_exports": (
                None if self.required_exports is None else sorted(self.required_exports)
            ),
            "canonical_shared": str(self.runtime_wasm),
            "canonical_reloc": str(self.runtime_reloc_wasm),
            "generation_manifest": str(self.generation_manifest),
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

    def accept_generation(
        self, *, observed_generation: RuntimeWasmGeneration | None = None
    ) -> RuntimeWasmGeneration | None:
        self.accepted_generation = None
        self.admission_report = None
        identity = self.pre_identity
        if identity is None:
            return None
        generation = observed_generation
        if generation is None:
            generation = read_runtime_wasm_generation(
                self.generation_manifest,
                expected_shared_identity=identity.shared,
                expected_reloc_identity=identity.reloc,
            )
        if generation is None or (
            generation.shared_identity != identity.shared
            or generation.reloc_identity != identity.reloc
        ):
            return None
        self.admission_report = runtime_wasm_generation_admission(
            generation, self.required_exports
        )
        if not self.admission_report.accepted:
            return None
        if not _select_runtime_wasm_generation(
            self.runtime_state, project_root=self.project_root, generation=generation
        ):
            return None
        self.accepted_generation = generation
        return generation

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
            / identity.shared.family_digest
            / uuid.uuid4().hex
        )
        root.mkdir(parents=True, exist_ok=False)
        self.staging_root = root
        self.staging_shared = root / self.runtime_wasm.name
        self.staging_reloc = root / self.runtime_reloc_wasm.name

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
            return _materialize_runtime_wasm_member_from_target(
                self.staging_member(reloc=reloc),
                reloc=reloc,
                json_output=self.json_output,
                cargo_timeout=self.cargo_timeout,
                project_root=self.project_root,
                resolved_modules=self.resolved_modules,
                required_exports=self.required_exports,
                spec=self.reloc_spec if reloc else self.shared_spec,
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


def _resolve_runtime_wasm_pair_identity(
    ctx: _RuntimeWasmPairBuild,
    shared_spec: _RuntimeWasmBuildSpec,
    reloc_spec: _RuntimeWasmBuildSpec,
    *,
    mode: Literal["pre_build", "post_build"],
) -> _RuntimeWasmPairIdentity:
    shared, reloc = _timed_runtime_identity_phase(
        phase="runtime_family_identity",
        mode=mode,
        operation=lambda: _resolved_runtime_wasm_family_identities(
            ctx.project_root,
            shared_spec,
            reloc_spec,
            build_python_admission=ctx.runtime_state.build_python_admission,
        ),
        identity_tree=_runtime_source_identity_tree,
    )
    return _RuntimeWasmPairIdentity(shared.toolchain_manifest, shared, reloc)


def _prepare_runtime_wasm_pair_build(
    runtime_state: _RuntimeArtifactState,
    *,
    json_output: bool,
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
) -> _RuntimeWasmPairBuild | None:
    runtime_wasm = runtime_state.runtime_wasm
    runtime_reloc_wasm = runtime_state.runtime_reloc_wasm
    if runtime_wasm is None or runtime_reloc_wasm is None:
        reason = "Runtime WASM shared/reloc artifact path is unavailable."
        record_runtime_wasm_failure(
            runtime_state,
            project_root=project_root,
            stage="pair-preparation",
            summary=reason,
        )
        return None

    def spec(path: Path, *, reloc: bool) -> _RuntimeWasmBuildSpec:
        return _compute_runtime_wasm_build_spec(
            project_root,
            path,
            reloc=reloc,
            cargo_profile=cargo_profile,
            simd_enabled=simd_enabled,
            freestanding=freestanding,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
            required_link_features=required_link_features,
            required_exports=required_exports,
            full_export_surface=full_export_surface,
        )

    shared_spec = spec(runtime_wasm, reloc=False)
    reloc_spec = spec(runtime_reloc_wasm, reloc=True)
    toolchain_path = _runtime_wasm_toolchain_manifest_path(shared_spec)
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
        runtime_wasm,
        runtime_reloc_wasm,
        shared_spec,
        reloc_spec,
        toolchain_path,
        runtime_wasm_generation_path(runtime_wasm),
        None,
    )
    try:
        shared_spec, reloc_spec = _resolve_runtime_wasm_cargo_specs(
            project_root,
            shared_spec,
            reloc_spec,
            simd_enabled=simd_enabled,
            freestanding=freestanding,
        )
        ctx.shared_spec, ctx.reloc_spec = shared_spec, reloc_spec
        ctx.pre_identity = _resolve_runtime_wasm_pair_identity(
            ctx,
            shared_spec,
            reloc_spec,
            mode="pre_build",
        )
        _bind_exact_family_fingerprints(ctx, ctx.pre_identity)
        ctx.pre_identity.toolchain.write(ctx.toolchain_manifest_path)
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        ctx.fail(
            "identity-provisioning",
            f"Runtime WASM identity provisioning failed: {exc}",
        )
        return None
    return ctx


def _materialize_runtime_wasm_pair(
    ctx: _RuntimeWasmPairBuild,
) -> _PairBuildOutcome:
    if ctx.accept_generation():
        return _PairBuildOutcome.ACCEPTED
    if ctx.pre_identity is None:
        return _PairBuildOutcome.FAILED
    assert ctx.pre_identity is not None
    hydrated = hydrate_runtime_wasm_pair_from_shared_cache(
        dest_shared=ctx.runtime_wasm,
        dest_reloc=ctx.runtime_reloc_wasm,
        shared_identity=ctx.pre_identity.shared,
        reloc_identity=ctx.pre_identity.reloc,
    )
    if hydrated is not None:
        if ctx.accept_generation(observed_generation=hydrated):
            return _PairBuildOutcome.ACCEPTED
        ctx.fail(
            "shared-cache-hydration",
            "Runtime WASM shared cache hydrated a pair that failed generation validation: "
            + json.dumps(ctx.generation_rejection_details(), sort_keys=True),
        )
        return _PairBuildOutcome.FAILED
    if reason := source_build_disabled_reason("Runtime WASM pair"):
        ctx.fail("rebuild-policy", reason)
        return _PairBuildOutcome.FAILED
    try:
        ctx.provision_staging()
    except (OSError, ValueError) as exc:
        ctx.fail(
            "pair-staging",
            f"Runtime WASM pair staging failed: {exc}",
        )
        return _PairBuildOutcome.FAILED
    if not _prepopulate_combined_runtime_wasm_target(
        runtime_state=ctx.runtime_state,
        shared_spec=ctx.shared_spec,
        reloc_spec=ctx.reloc_spec,
        json_output=ctx.json_output,
        cargo_timeout=ctx.cargo_timeout,
        project_root=ctx.project_root,
        simd_enabled=ctx.simd_enabled,
        freestanding=ctx.freestanding,
        force_build=False,
    ):
        return _PairBuildOutcome.FAILED
    if not ctx.ensure_member(reloc=False):
        if ctx.runtime_state.runtime_wasm_build_failure is None:
            ctx.fail(
                "shared-member-publication",
                "Runtime WASM combined target could not publish the shared member.",
            )
        return _PairBuildOutcome.FAILED
    if not ctx.ensure_member(reloc=True):
        if ctx.runtime_state.runtime_wasm_build_failure is None:
            ctx.fail(
                "reloc-member-publication",
                "Runtime WASM combined target could not publish the relocatable member.",
            )
        return _PairBuildOutcome.FAILED
    return _PairBuildOutcome.BUILT


def _publish_runtime_wasm_pair(ctx: _RuntimeWasmPairBuild) -> bool:
    post_shared = ctx.shared_spec
    post_reloc = ctx.reloc_spec
    try:
        plan = ctx.shared_spec.cargo_plan
        if plan is None:
            raise ValueError("runtime WASM build lost its resolved Cargo plan")
        post_shared = post_shared.with_cargo_plan(plan)
        post_reloc = post_reloc.with_cargo_plan(plan)
        post_identity = _resolve_runtime_wasm_pair_identity(
            ctx, post_shared, post_reloc, mode="post_build"
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
        published = publish_runtime_wasm_generation(
            ctx.runtime_wasm,
            ctx.runtime_reloc_wasm,
            shared_identity=post_identity.shared,
            reloc_identity=post_identity.reloc,
            source_shared=ctx.staging_member(reloc=False),
            source_reloc=ctx.staging_member(reloc=True),
        )
    except (OSError, ValueError) as exc:
        return ctx.fail(
            "generation-publication",
            f"Runtime WASM pair publication failed: {exc}",
        )
    if not post_shared.incremental_enabled:
        _warn_runtime_wasm_cache_publish_failure(
            publish_runtime_wasm_pair_to_shared_cache(
                shared=published.shared,
                reloc=published.reloc,
                shared_identity=post_identity.shared,
                reloc_identity=post_identity.reloc,
            ),
            json_output=ctx.json_output,
        )
    if ctx.accept_generation(observed_generation=published):
        return True
    return ctx.fail(
        "generation-acceptance",
        "Published Runtime WASM generation failed immutable acceptance: "
        + json.dumps(ctx.generation_rejection_details(), sort_keys=True),
    )


def _ensure_installed_runtime_wasm(
    runtime_state: _RuntimeArtifactState,
    cell: InstalledRuntimeCell,
    *,
    project_root: Path,
    required_link_features: frozenset[str],
    required_exports: set[str] | frozenset[str] | None,
    planned_exports: set[str] | frozenset[str] | None,
    bind_for_codegen: bool,
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
    report = runtime_wasm_generation_admission(generation, exports)
    if not report.accepted:
        return fail(
            "installed-runtime-exports",
            f"Installed runtime WASM cell {cell.id} failed runtime admission: "
            + json.dumps(report.details(), sort_keys=True),
        )
    if not _select_runtime_wasm_generation(
        runtime_state, project_root=project_root, generation=generation
    ):
        return fail(
            "installed-runtime-receipt",
            "Cannot publish the installed runtime WASM expected-identity receipt.",
        )
    runtime_state.runtime_wasm = generation.shared.with_name("molt_runtime.wasm")
    runtime_state.runtime_reloc_wasm = generation.reloc.with_name(
        "molt_runtime_reloc.wasm"
    )
    if bind_for_codegen and binding is None:
        try:
            binding = bind_runtime_wasm_codegen(generation, planned_exports)
        except (OSError, ValueError) as exc:
            return fail(
                "codegen-binding", f"Runtime WASM codegen binding failed: {exc}"
            )
        runtime_state.runtime_wasm_codegen_binding = binding
        runtime_state.runtime_wasm_generation = binding.generation.manifest
    return True


def _ensure_runtime_wasm_both(
    runtime_state: _RuntimeArtifactState,
    *,
    json_output: bool,
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
) -> bool:
    runtime_state.runtime_wasm_build_failure = None
    binding = runtime_state.runtime_wasm_codegen_binding
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
        )
    with build_python_scope(runtime_state):
        ctx = _prepare_runtime_wasm_pair_build(
            runtime_state,
            json_output=json_output,
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
        )
        if ctx is None:
            return False
        try:
            if binding is not None:
                generation = binding.generation
                if ctx.pre_identity is None or (
                    ctx.pre_identity.shared != generation.shared_identity
                    or ctx.pre_identity.reloc != generation.reloc_identity
                ):
                    return ctx.fail(
                        "codegen-identity-stability",
                        "Runtime WASM build inputs changed after app layout was bound; "
                        "refusing runtime reselection beneath compiled code.",
                    )
                ctx.generation_manifest = generation.manifest
                ctx.required_exports = required_exports
                try:
                    binding.verify()
                except (OSError, ValueError) as exc:
                    return ctx.fail(
                        "codegen-identity-stability",
                        f"The bound runtime WASM generation changed: {exc}",
                    )
                if ctx.accept_generation(observed_generation=generation):
                    return True
                return ctx.fail(
                    "codegen-generation-admission",
                    "The runtime WASM pair bound before app code generation no longer "
                    "satisfies its emitted import contract: "
                    + json.dumps(ctx.generation_rejection_details(), sort_keys=True),
                )
            outcome = _materialize_runtime_wasm_pair(ctx)
            if outcome is _PairBuildOutcome.FAILED:
                return False
            if outcome is _PairBuildOutcome.BUILT and not _publish_runtime_wasm_pair(
                ctx
            ):
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
                    binding = bind_runtime_wasm_codegen(generation, planned_exports)
                except (OSError, ValueError) as exc:
                    return ctx.fail(
                        "codegen-binding", f"Runtime WASM codegen binding failed: {exc}"
                    )
                runtime_state.runtime_wasm_codegen_binding = binding
                runtime_state.runtime_wasm_generation = binding.generation.manifest
            return True
        finally:
            ctx.cleanup_staging()
