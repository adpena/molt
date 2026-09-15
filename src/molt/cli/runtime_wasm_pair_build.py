from __future__ import annotations

import contextlib
import json
import os
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass
from enum import Enum, auto
from pathlib import Path
from typing import Literal

from molt._wasm_runtime_exports import (
    wasm_runtime_missing_required_exports,
    wasm_runtime_required_export_symbol_kinds,
)
from molt.cli.artifact_state import (
    _build_state_root,
    _runtime_target_fingerprint_path,
)
from molt.cli.config_resolution import (
    DEFAULT_RUNTIME_STDLIB_PROFILE,
)
from molt.cli.cargo_execution import (
    CargoPlanExecutionError,
    cargo_execution_evidence,
    _text_output,
)
from molt.cli.models import (
    _RuntimeArtifactState,
)
from molt.cli.runtime_artifact_selection import (
    RuntimeCrateType,
)
from molt.cli.runtime_build_identity import (
    RuntimeBuildIdentity,
    RuntimeToolchainContentManifest,
    runtime_build_fingerprint,
)
from molt.cli.runtime_fingerprints import (
    _write_runtime_fingerprint,
)
from molt.cli.runtime_wasm_build import _materialize_runtime_wasm_member_from_target
from molt.cli.runtime_wasm_failure import record_runtime_wasm_failure
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
    _runtime_exports_satisfy_for_mode,
    _runtime_missing_exports_for_mode,
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
from molt.cli.runtime_wasm_generation import (
    RuntimeWasmGeneration,
    RuntimeWasmExpectedPair,
    bind_runtime_wasm_codegen,
    publish_runtime_wasm_generation,
    read_runtime_wasm_generation,
    runtime_wasm_generation_path,
)
from molt.cli.runtime_wasm_validation import (
    _is_valid_runtime_wasm_artifact,
    _is_valid_shared_runtime_wasm_artifact,
    _runtime_wasm_artifact_validation_error,
    _shared_runtime_wasm_validation_error,
)
from molt.wasm_artifact import (
    inspect_wasm_binary as _inspect_wasm_binary,
)
from molt.wasm_linking_symbols import wasm_linking_defined_names


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
    plan.verify()
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
        print(
            "Building runtime wasm (single combined compile: staticlib+cdylib)...",
            file=sys.stderr,
        )
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
        return ctx.fail(
            "combined-cargo",
            "Runtime wasm combined build failed",
            build=build,
            command=tuple(cmd),
            timed_out=build.returncode == 124,
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

    def reloc_missing_required_symbols(self, path: Path) -> set[str]:
        expected_kinds = wasm_runtime_required_export_symbol_kinds(
            self.required_exports
        )
        available = wasm_linking_defined_names(path, expected_kinds)
        return wasm_runtime_missing_required_exports(
            available,
            self.required_exports,
        )

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
        self, *, expected_generation: RuntimeWasmGeneration | None = None
    ) -> bool:
        identity = self.pre_identity
        if identity is None:
            return False
        generation = read_runtime_wasm_generation(
            self.generation_manifest,
            expected_shared_identity=identity.shared,
            expected_reloc_identity=identity.reloc,
        )
        if generation is None:
            return False
        if expected_generation is not None and (
            generation.shared != expected_generation.shared
            or generation.reloc != expected_generation.reloc
        ):
            return False
        try:
            rejected = (
                not _is_valid_shared_runtime_wasm_artifact(generation.shared)
                or not _is_valid_runtime_wasm_artifact(generation.reloc)
                or not _runtime_exports_satisfy_for_mode(
                    generation.shared, self.required_exports, reloc=False
                )
                or bool(self.reloc_missing_required_symbols(generation.reloc))
            )
        except (OSError, UnicodeDecodeError, ValueError):
            return False
        if rejected:
            return False
        expected_path = (
            _build_state_root(self.project_root)
            / "runtime_wasm_generations"
            / f"{identity.shared.family_digest}.expected.json"
        )
        try:
            RuntimeWasmExpectedPair(identity.shared, identity.reloc).write(
                expected_path
            )
        except (OSError, ValueError):
            return False
        self.runtime_state.runtime_wasm_generation = generation.manifest
        self.runtime_state.runtime_wasm_selected = generation.shared
        self.runtime_state.runtime_reloc_wasm_selected = generation.reloc
        self.runtime_state.runtime_wasm_expected_identity = expected_path
        return True

    def generation_rejection_details(self) -> dict[str, object]:
        identity = self.pre_identity
        if identity is None:
            return {"generation": "missing expected pair identity"}
        generation = read_runtime_wasm_generation(
            self.generation_manifest,
            expected_shared_identity=identity.shared,
            expected_reloc_identity=identity.reloc,
        )
        if generation is None:
            return {
                "generation": "manifest, member content, or identity validation failed"
            }
        shared_error = _shared_runtime_wasm_validation_error(generation.shared)
        reloc_error = _runtime_wasm_artifact_validation_error(generation.reloc)
        shared_missing = _runtime_missing_exports_for_mode(
            generation.shared, self.required_exports, reloc=False
        )
        try:
            reloc_missing = self.reloc_missing_required_symbols(generation.reloc)
            reloc_linking_error = None
        except (OSError, UnicodeDecodeError, ValueError) as exc:
            reloc_missing = set()
            reloc_linking_error = str(exc)
        return {
            "generation": str(generation.manifest),
            "shared": str(generation.shared),
            "shared_validation_error": shared_error,
            "shared_missing_exports": sorted(shared_missing),
            "reloc": str(generation.reloc),
            "reloc_linking_error": reloc_linking_error,
            "reloc_validation_error": reloc_error,
            "reloc_missing_symbols": sorted(reloc_missing),
        }

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
    if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") == "1":
        ctx.fail(
            "rebuild-policy",
            "Runtime WASM pair is unavailable and MOLT_SKIP_RUNTIME_REBUILD=1.",
        )
        return _PairBuildOutcome.FAILED
    if ctx.pre_identity is None:
        return _PairBuildOutcome.FAILED
    if ctx.accept_generation():
        return _PairBuildOutcome.ACCEPTED
    assert ctx.pre_identity is not None
    if hydrate_runtime_wasm_pair_from_shared_cache(
        dest_shared=ctx.runtime_wasm,
        dest_reloc=ctx.runtime_reloc_wasm,
        shared_identity=ctx.pre_identity.shared,
        reloc_identity=ctx.pre_identity.reloc,
        is_valid_shared=_is_valid_shared_runtime_wasm_artifact,
        is_valid_reloc=_is_valid_runtime_wasm_artifact,
    ):
        if ctx.accept_generation():
            return _PairBuildOutcome.ACCEPTED
        ctx.fail(
            "shared-cache-hydration",
            "Runtime WASM shared cache hydrated a pair that failed generation validation.",
        )
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
        plan.verify()
        post_shared = post_shared.with_cargo_plan(plan)._replace(
            link_inputs=ctx.shared_spec.link_inputs
        )
        post_reloc = post_reloc.with_cargo_plan(plan)._replace(
            link_inputs=ctx.reloc_spec.link_inputs
        )
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
    if ctx.accept_generation():
        return True
    return ctx.fail(
        "generation-acceptance",
        "Published Runtime WASM generation failed immutable acceptance: "
        + json.dumps(ctx.generation_rejection_details(), sort_keys=True),
    )


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
            if ctx.accept_generation(expected_generation=generation):
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
        if outcome is _PairBuildOutcome.BUILT and not _publish_runtime_wasm_pair(ctx):
            return False
        if bind_for_codegen:
            assert ctx.pre_identity is not None
            generation = read_runtime_wasm_generation(
                ctx.generation_manifest,
                expected_shared_identity=ctx.pre_identity.shared,
                expected_reloc_identity=ctx.pre_identity.reloc,
            )
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
