from __future__ import annotations

import contextlib
import functools
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import uuid
from concurrent.futures import ThreadPoolExecutor
from typing import Any, Collection, Literal, Mapping, NamedTuple, Sequence

from molt._runtime_feature_gates import link_affecting_feature_gate_for_symbol
from molt._wasm_runtime_exports import (
    wasm_cpython_abi_requested_data_export_names,
    wasm_cpython_abi_requested_export_names,
    wasm_split_runtime_export_rename_map,
    wasm_runtime_export_link_args,
    wasm_runtime_shared_export_link_args,
)
from molt.cli.artifact_state import (
    _artifact_state_path_for_build_state_root,
    _build_state_root,
    _canonical_build_state_root,
    _canonical_target_root,
    _maybe_hydrate_artifact_from_canonical_target,
    _runtime_fingerprint_path,
    _runtime_target_fingerprint_path,
)
from molt.cli.atomic_io import (
    _atomic_copy_file,
    _atomic_write_bytes,
    _atomic_write_text,
)
from molt.cli.build_locks import _build_lock
from molt.cli.capability_spec import _dedupe_preserve_order
from molt.cli.config_resolution import (
    DEFAULT_RUNTIME_STDLIB_PROFILE,
    DEFAULT_STDLIB_PROFILE,
)
from molt.cli.cargo_execution import (
    _build_slot,
    _cargo_build_env,
    _maybe_enable_sccache,
    _run_cargo_with_sccache_retry,
)
from molt.cli.cargo_profiles import _CARGO_PROFILE_NAME_RE, _resolve_cargo_profile_name
from molt.cli.command_runtime import (
    _run_completed_command,
    _run_subprocess_captured_to_tempfiles,
)
from molt.cli.compiler_metadata import _compiler_root
from molt.cli.file_hashing import _sha256_file
from molt.cli.native_link_manifest import (
    NativeLinkDependencyManifestError,
    native_link_dependency_manifest_path,
    read_native_link_dependency_manifest,
    write_native_link_dependency_manifest,
)
from molt.cli.runtime_features import (
    _runtime_builtin_features_for_profile,
    _runtime_cargo_features,
    _wasm_runtime_feature_plan,
    runtime_fingerprint_features_for_profile,
    runtime_cargo_feature_for_profile,
    runtime_stdlib_profile_for_required_features,
)
from molt.cli.runtime_fingerprints import (
    _read_runtime_fingerprint,
    _refresh_runtime_fingerprint_metadata,
    _runtime_artifact_fingerprint_matches,
    _runtime_fingerprint,
    _runtime_fingerprint_metadata_needs_refresh,
    _write_runtime_fingerprint,
)
from molt.cli.runtime_paths import (
    _cargo_profile_dir,
    _cargo_target_root,
    _runtime_cargo_scratch_lib_path,
    _runtime_lib_path,
    _runtime_wasm_artifact_path,
)
from molt.cli.runtime_wasm_cache import (
    _build_reuse_compatible_enabled,
    _hydrate_runtime_wasm_from_compatible_cache,
    _hydrate_runtime_wasm_from_shared_cache,
    _publish_runtime_wasm_to_shared_cache,
    _runtime_wasm_compat_digest,
)
from molt.cli.runtime_wasm_build_timings import (
    _record_runtime_wasm_build_phase,
    _record_runtime_wasm_longdouble_archives,
    _runtime_wasm_build_timings_snapshot,
)
from molt.cli.runtime_wasm_validation import (
    _is_valid_runtime_wasm_artifact,
    _is_valid_shared_runtime_wasm_artifact,
    _split_runtime_wasm_exports_satisfy,
    _split_runtime_wasm_missing_exports,
    _runtime_wasm_exports_satisfy,
    _runtime_wasm_has_matching_integrity_pin,
    _runtime_wasm_integrity_key,
    _runtime_wasm_missing_exports,
    _write_runtime_wasm_integrity_sidecar,
)
from molt.cli.wasm_link_args import (
    wasm_link_args_from_rustflags as _wasm_link_args_from_rustflags,
    wasm_link_args_response_file as _wasm_link_args_response_file,
    write_wasm_link_args_response_file as _write_wasm_link_args_response_file,
)
from molt.cli import wasm_toolchain
from molt.cli.models import BuildProfile, _RuntimeArtifactState
from molt.wasm_artifact import (
    inspect_wasm_binary as _inspect_wasm_binary,
    rename_wasm_export_names,
    strip_wasm_publication_sections,
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


_RUNTIME_LIB_VERIFIED: set[
    tuple[
        str,
        str,
        str,
        str,
        str | None,
        str,
        tuple[str, ...],
        tuple[str | None, str | None, str | None, str | None],
    ]
] = set()
_NATIVE_RUNTIME_READY_EXECUTOR: ThreadPoolExecutor | None = None


def _record_runtime_build_stage_ms(
    stage_timings_ms: dict[str, float] | None,
    name: str,
    started_at: float,
) -> None:
    if stage_timings_ms is None:
        return
    stage_timings_ms[name] = round(
        max(0.0, (time.perf_counter() - started_at) * 1000.0),
        6,
    )


def _initialize_runtime_artifact_state(
    *,
    is_rust_transpile: bool,
    is_wasm: bool,
    emit_mode: str,
    molt_root: Path,
    runtime_cargo_profile: str,
    target_triple: str | None,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    extra_runtime_features: Sequence[str] | None = None,
) -> _RuntimeArtifactState:
    state = _RuntimeArtifactState(
        extra_runtime_features=tuple(
            _dedupe_preserve_order(
                feature.strip()
                for feature in (extra_runtime_features or ())
                if feature and feature.strip()
            )
        )
    )
    if is_rust_transpile:
        return state
    if is_wasm:
        state.runtime_wasm = _runtime_wasm_artifact_path(molt_root, "molt_runtime.wasm")
        state.runtime_reloc_wasm = _runtime_wasm_artifact_path(
            molt_root, "molt_runtime_reloc.wasm"
        )
        return state
    if emit_mode in {"bin", "obj"}:
        state.runtime_lib = _runtime_lib_path(
            molt_root,
            runtime_cargo_profile,
            target_triple,
            stdlib_profile=stdlib_profile,
        )
    return state


def _native_runtime_ready_executor() -> ThreadPoolExecutor:
    global _NATIVE_RUNTIME_READY_EXECUTOR
    if _NATIVE_RUNTIME_READY_EXECUTOR is None:
        _NATIVE_RUNTIME_READY_EXECUTOR = ThreadPoolExecutor(
            max_workers=1,
            thread_name_prefix="molt-runtime-ready",
        )
    return _NATIVE_RUNTIME_READY_EXECUTOR


def _maybe_start_native_runtime_lib_ready_async(
    runtime_state: _RuntimeArtifactState,
    *,
    target_triple: str | None,
    json_output: bool,
    runtime_cargo_profile: str,
    molt_root: Path,
    cargo_timeout: float | None,
    diagnostics_enabled: bool,
    phase_starts: dict[str, float] | None,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: set[str] | frozenset[str] | None = None,
) -> None:
    runtime_lib = runtime_state.runtime_lib
    if runtime_lib is None or runtime_state.runtime_lib_ready_future is not None:
        return
    if (
        diagnostics_enabled
        and phase_starts is not None
        and "runtime_setup" not in phase_starts
    ):
        phase_starts["runtime_setup"] = time.perf_counter()
    runtime_state.runtime_lib_ready_future = _native_runtime_ready_executor().submit(
        _ensure_runtime_lib_ready,
        runtime_state,
        target_triple=target_triple,
        json_output=json_output,
        runtime_cargo_profile=runtime_cargo_profile,
        molt_root=molt_root,
        cargo_timeout=cargo_timeout,
        stdlib_profile=stdlib_profile,
        resolved_modules=resolved_modules,
    )


def _ensure_runtime_lib_ready(
    runtime_state: _RuntimeArtifactState,
    *,
    target_triple: str | None,
    json_output: bool,
    runtime_cargo_profile: str,
    molt_root: Path,
    cargo_timeout: float | None,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: Collection[str] | None = None,
    stage_timings_ms: dict[str, float] | None = None,
) -> bool:
    runtime_lib = runtime_state.runtime_lib
    if runtime_lib is None:
        return True
    return _ensure_runtime_lib(
        runtime_lib,
        target_triple,
        json_output,
        runtime_cargo_profile,
        molt_root,
        cargo_timeout,
        stdlib_profile=stdlib_profile,
        resolved_modules=resolved_modules,
        extra_runtime_features=runtime_state.extra_runtime_features,
        stage_timings_ms=stage_timings_ms,
        runtime_state=runtime_state,
    )


def _ensure_native_runtime_lib_ready_before_link(
    runtime_state: _RuntimeArtifactState,
    *,
    target_triple: str | None,
    json_output: bool,
    runtime_cargo_profile: str,
    molt_root: Path,
    cargo_timeout: float | None,
    diagnostics_enabled: bool,
    phase_starts: dict[str, float],
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: set[str] | frozenset[str] | None = None,
    stage_timings_ms: dict[str, float] | None = None,
) -> bool:
    runtime_lib = runtime_state.runtime_lib
    if runtime_lib is None:
        return True
    if runtime_state.runtime_lib_ready_future is not None:
        if diagnostics_enabled and "runtime_setup" not in phase_starts:
            phase_starts["runtime_setup"] = time.perf_counter()
        try:
            ready = bool(runtime_state.runtime_lib_ready_future.result())
            return ready and runtime_state.native_link_source_fingerprint is not None
        finally:
            runtime_state.runtime_lib_ready_future = None
    if diagnostics_enabled and "runtime_setup" not in phase_starts:
        phase_starts["runtime_setup"] = time.perf_counter()
    ready = _ensure_runtime_lib_ready(
        runtime_state,
        target_triple=target_triple,
        json_output=json_output,
        runtime_cargo_profile=runtime_cargo_profile,
        molt_root=molt_root,
        cargo_timeout=cargo_timeout,
        stdlib_profile=stdlib_profile,
        resolved_modules=resolved_modules,
        stage_timings_ms=stage_timings_ms,
    )
    return ready and runtime_state.native_link_source_fingerprint is not None


def _runtime_lib_verified_session_key(
    *,
    project_root: Path,
    runtime_lib: Path,
    fingerprint_path: Path,
    cargo_profile: str,
    target_triple: str | None,
    rustflags: str,
    fingerprint_features: tuple[str, ...],
    fingerprint: Mapping[str, str | None] | None,
) -> (
    tuple[
        str,
        str,
        str,
        str,
        str | None,
        str,
        tuple[str, ...],
        tuple[str | None, str | None, str | None, str | None],
    ]
    | None
):
    if fingerprint is None:
        return None
    return (
        os.fspath(project_root),
        os.fspath(runtime_lib),
        os.fspath(fingerprint_path),
        cargo_profile,
        target_triple,
        rustflags,
        fingerprint_features,
        (
            fingerprint.get("hash"),
            fingerprint.get("rustc"),
            fingerprint.get("inputs_digest"),
            fingerprint.get("meta_digest"),
        ),
    )


def _native_runtime_cargo_command(
    *,
    cargo_profile: str,
    concrete_stdlib_profile: str,
    runtime_features: Sequence[str],
    builtin_features: Sequence[str],
    concrete_stdlib_feature: str,
    target_triple: str | None,
) -> list[str]:
    """Return the one exact Cargo command used for build and manifest refresh."""
    cmd = [
        "cargo",
        "rustc",
        "-p",
        "molt-runtime",
        "--profile",
        cargo_profile,
        "--message-format=json-render-diagnostics",
    ]
    if concrete_stdlib_profile != "full":
        cmd.append("--no-default-features")
        concrete_features = _dedupe_preserve_order(
            list(runtime_features) + list(builtin_features) + [concrete_stdlib_feature]
        )
        cmd.extend(["--features", ",".join(concrete_features)])
    elif target_triple and "wasm" in target_triple:
        cmd.append("--no-default-features")
        wasm_features = list(runtime_features) + [
            "stdlib_crypto",
            "stdlib_compression",
            "stdlib_serialization",
            "stdlib_archive",
            "stdlib_ast",
            "stdlib_fs_extra",
            "builtin_set",
            "builtin_complex",
            "builtin_memoryview",
            "builtin_contextvars",
            "builtin_fcntl",
        ]
        cmd.extend(["--features", ",".join(wasm_features)])
    else:
        full_features = _dedupe_preserve_order(
            list(runtime_features) + [concrete_stdlib_feature]
        )
        cmd.extend(["--features", ",".join(full_features)])
    if target_triple:
        cmd.extend(["--target", target_triple])
    cmd.extend(["--", "--print", "native-static-libs"])
    return cmd


def _native_link_manifest_matches(
    runtime_lib: Path,
    *,
    cargo_profile: str,
    target_triple: str | None,
    source_root: Path,
    source_fingerprint: Mapping[str, object],
) -> bool:
    try:
        read_native_link_dependency_manifest(
            runtime_lib,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            source_root=source_root,
            source_fingerprint=source_fingerprint,
        )
    except NativeLinkDependencyManifestError:
        return False
    return True


def _native_link_source_fingerprint(
    fingerprint: Mapping[str, object] | None,
) -> dict[str, object] | None:
    """Project the runtime fingerprint into the native-link source identity."""
    if fingerprint is None:
        return None
    projected = {
        key: fingerprint.get(key)
        for key in ("hash", "inputs_digest", "meta_digest", "rustc")
    }
    if any(
        not isinstance(projected[key], str) or not projected[key]
        for key in ("hash", "meta_digest", "rustc")
    ):
        return None
    inputs_digest = projected["inputs_digest"]
    if inputs_digest is not None and (
        not isinstance(inputs_digest, str) or not inputs_digest
    ):
        return None
    return projected


def _runtime_archive_bytes_match(left: Path, right: Path) -> bool:
    try:
        return left.stat().st_size == right.stat().st_size and _sha256_file(
            left
        ) == _sha256_file(right)
    except OSError:
        return False


def _refresh_native_link_manifest(
    *,
    runtime_lib: Path,
    target_triple: str | None,
    cargo_profile: str,
    project_root: Path,
    cmd: list[str],
    build_env: dict[str, str],
    cargo_timeout: float | None,
    json_output: bool,
    source_fingerprint: Mapping[str, object],
) -> bool:
    """Refresh missing provenance through the exact no-op-capable Cargo command."""
    try:
        with _build_slot() as _slot:
            result = _run_cargo_with_sccache_retry(
                cmd,
                cwd=project_root,
                env=build_env,
                timeout=cargo_timeout,
                json_output=json_output,
                label="Runtime native-link manifest refresh",
            )
    except subprocess.TimeoutExpired:
        return False
    if result.returncode != 0:
        if not json_output:
            detail = result.stderr.strip() or result.stdout.strip()
            if detail:
                print(detail, file=sys.stderr)
        return False
    cargo_runtime_lib = _runtime_cargo_scratch_lib_path(runtime_lib, target_triple)
    if not _runtime_archive_bytes_match(runtime_lib, cargo_runtime_lib):
        if not json_output:
            print(
                "Runtime native-link manifest refresh produced an archive that "
                "does not match the selected runtime artifact.",
                file=sys.stderr,
            )
        return False
    try:
        write_native_link_dependency_manifest(
            result.stdout,
            cargo_stderr=result.stderr,
            runtime_lib=runtime_lib,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            source_root=project_root,
            source_fingerprint=source_fingerprint,
        )
    except (OSError, NativeLinkDependencyManifestError) as exc:
        if not json_output:
            print(
                f"Failed to publish runtime native-link manifest: {exc}",
                file=sys.stderr,
            )
        return False
    return True


def _ensure_runtime_lib(
    runtime_lib: Path,
    target_triple: str | None,
    json_output: bool,
    cargo_profile: str,
    project_root: Path,
    cargo_timeout: float | None,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: Collection[str] | None = None,
    extra_runtime_features: Sequence[str] | None = None,
    stage_timings_ms: dict[str, float] | None = None,
    runtime_state: _RuntimeArtifactState | None = None,
) -> bool:
    if runtime_state is not None:
        runtime_state.native_link_source_fingerprint = None
    rustflags = os.environ.get("RUSTFLAGS", "")
    runtime_features = tuple(
        _dedupe_preserve_order(
            list(_runtime_cargo_features(target_triple))
            + list(extra_runtime_features or ())
        )
    )
    builtin_features = _runtime_builtin_features_for_profile(
        stdlib_profile,
        target_triple=target_triple,
    )
    # Cargo writes the platform staticlib name as scratch output. Molt then
    # materializes a profile-qualified link alias, so the requested feature
    # profile must remain an explicit fingerprint input.
    concrete_stdlib_profile = stdlib_profile or DEFAULT_RUNTIME_STDLIB_PROFILE
    concrete_stdlib_feature = runtime_cargo_feature_for_profile(concrete_stdlib_profile)
    fingerprint_features = runtime_fingerprint_features_for_profile(
        concrete_stdlib_profile,
        target_triple=target_triple,
        extra_runtime_features=extra_runtime_features,
    )
    cmd = _native_runtime_cargo_command(
        cargo_profile=cargo_profile,
        concrete_stdlib_profile=concrete_stdlib_profile,
        runtime_features=runtime_features,
        builtin_features=builtin_features,
        concrete_stdlib_feature=concrete_stdlib_feature,
        target_triple=target_triple,
    )
    build_env = _cargo_build_env()
    build_env["CARGO_TARGET_DIR"] = str(_cargo_target_root(project_root))
    _maybe_enable_sccache(build_env)
    fingerprint_path = _runtime_fingerprint_path(
        project_root, runtime_lib, cargo_profile, target_triple
    )
    read_fingerprint_start = time.perf_counter()
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    _record_runtime_build_stage_ms(
        stage_timings_ms,
        "runtime_lib_read_fingerprint",
        read_fingerprint_start,
    )
    compute_fingerprint_start = time.perf_counter()
    fingerprint = _runtime_fingerprint(
        project_root,
        cargo_profile=cargo_profile,
        target_triple=target_triple,
        rustflags=rustflags,
        runtime_features=fingerprint_features,
        stored_fingerprint=stored_fingerprint,
    )
    _record_runtime_build_stage_ms(
        stage_timings_ms,
        "runtime_lib_compute_fingerprint",
        compute_fingerprint_start,
    )
    source_fingerprint = _native_link_source_fingerprint(fingerprint)
    if source_fingerprint is None:
        if not json_output:
            print(
                "Failed to compute an exact source/toolchain identity for the "
                "runtime native-link manifest.",
                file=sys.stderr,
            )
        return False

    def accept_source_attestation() -> bool:
        if runtime_state is not None:
            runtime_state.native_link_source_fingerprint = dict(source_fingerprint)
        return True

    # The skip knob may avoid recompilation, but it may not bypass artifact/source
    # custody. Missing or foreign provenance is refreshed by the same exact Cargo
    # command from this invoking workspace and accepted only when bytes match.
    if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") == "1" and runtime_lib.exists():
        if _native_link_manifest_matches(
            runtime_lib,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            source_root=project_root,
            source_fingerprint=source_fingerprint,
        ):
            return accept_source_attestation()
        lock_target = target_triple or "native"
        with _build_lock(project_root, f"runtime.{cargo_profile}.{lock_target}"):
            refreshed = _refresh_native_link_manifest(
                runtime_lib=runtime_lib,
                target_triple=target_triple,
                cargo_profile=cargo_profile,
                project_root=project_root,
                cmd=cmd,
                build_env=build_env,
                cargo_timeout=cargo_timeout,
                json_output=json_output,
                source_fingerprint=source_fingerprint,
            )
            return accept_source_attestation() if refreshed else False
    session_key = _runtime_lib_verified_session_key(
        project_root=project_root,
        runtime_lib=runtime_lib,
        fingerprint_path=fingerprint_path,
        cargo_profile=cargo_profile,
        target_triple=target_triple,
        rustflags=rustflags,
        fingerprint_features=fingerprint_features,
        fingerprint=fingerprint,
    )
    if (
        session_key is not None
        and session_key in _RUNTIME_LIB_VERIFIED
        and _native_link_manifest_matches(
            runtime_lib,
            cargo_profile=cargo_profile,
            target_triple=target_triple,
            source_root=project_root,
            source_fingerprint=source_fingerprint,
        )
    ):
        return accept_source_attestation()
    lock_target = target_triple or "native"
    lock_name = f"runtime.{cargo_profile}.{lock_target}"
    with _build_lock(project_root, lock_name):
        if stored_fingerprint is None:
            reread_fingerprint_start = time.perf_counter()
            stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
            _record_runtime_build_stage_ms(
                stage_timings_ms,
                "runtime_lib_reread_fingerprint_in_lock",
                reread_fingerprint_start,
            )
        artifact_match_start = time.perf_counter()
        if _runtime_artifact_fingerprint_matches(
            runtime_lib,
            fingerprint,
            fingerprint_path,
            require_artifact_digest=True,
        ):
            if fingerprint is not None and _runtime_fingerprint_metadata_needs_refresh(
                stored_fingerprint, fingerprint
            ):
                with contextlib.suppress(OSError):
                    _refresh_runtime_fingerprint_metadata(
                        fingerprint_path,
                        fingerprint,
                    )
            _record_runtime_build_stage_ms(
                stage_timings_ms,
                "runtime_lib_artifact_match",
                artifact_match_start,
            )
            if not _native_link_manifest_matches(
                runtime_lib,
                cargo_profile=cargo_profile,
                target_triple=target_triple,
                source_root=project_root,
                source_fingerprint=source_fingerprint,
            ) and not _refresh_native_link_manifest(
                runtime_lib=runtime_lib,
                target_triple=target_triple,
                cargo_profile=cargo_profile,
                project_root=project_root,
                cmd=cmd,
                build_env=build_env,
                cargo_timeout=cargo_timeout,
                json_output=json_output,
                source_fingerprint=source_fingerprint,
            ):
                return False
            if session_key is not None:
                _RUNTIME_LIB_VERIFIED.add(session_key)
            return accept_source_attestation()
        _record_runtime_build_stage_ms(
            stage_timings_ms,
            "runtime_lib_artifact_match",
            artifact_match_start,
        )
        canonical_target_root = _canonical_target_root(project_root)
        profile_dir = _cargo_profile_dir(cargo_profile)
        if target_triple:
            canonical_runtime_lib = (
                canonical_target_root / target_triple / profile_dir / runtime_lib.name
            )
        else:
            canonical_runtime_lib = (
                canonical_target_root / profile_dir / runtime_lib.name
            )
        target_label = (
            (target_triple or "native").replace(os.sep, "_").replace(":", "_")
        )
        canonical_fingerprint_path = _artifact_state_path_for_build_state_root(
            _canonical_build_state_root(project_root),
            canonical_runtime_lib,
            subdir="runtime_fingerprints",
            stem_suffix=f"{cargo_profile}.{target_label}",
            extension="fingerprint",
        )
        hydrate_start = time.perf_counter()
        if _maybe_hydrate_artifact_from_canonical_target(
            artifact=runtime_lib,
            fingerprint=fingerprint,
            fingerprint_path=fingerprint_path,
            candidate_artifact=canonical_runtime_lib,
            candidate_fingerprint_path=canonical_fingerprint_path,
            require_artifact_digest=True,
        ):
            _record_runtime_build_stage_ms(
                stage_timings_ms,
                "runtime_lib_canonical_hydrate",
                hydrate_start,
            )
            manifest_hydrated = False
            try:
                read_native_link_dependency_manifest(
                    canonical_runtime_lib,
                    cargo_profile=cargo_profile,
                    target_triple=target_triple,
                    source_root=project_root,
                    source_fingerprint=source_fingerprint,
                )
                _atomic_copy_file(
                    native_link_dependency_manifest_path(canonical_runtime_lib),
                    native_link_dependency_manifest_path(runtime_lib),
                )
                manifest_hydrated = _native_link_manifest_matches(
                    runtime_lib,
                    cargo_profile=cargo_profile,
                    target_triple=target_triple,
                    source_root=project_root,
                    source_fingerprint=source_fingerprint,
                )
            except (OSError, NativeLinkDependencyManifestError):
                manifest_hydrated = False
            if not manifest_hydrated and not _refresh_native_link_manifest(
                runtime_lib=runtime_lib,
                target_triple=target_triple,
                cargo_profile=cargo_profile,
                project_root=project_root,
                cmd=cmd,
                build_env=build_env,
                cargo_timeout=cargo_timeout,
                json_output=json_output,
                source_fingerprint=source_fingerprint,
            ):
                return False
            if session_key is not None:
                _RUNTIME_LIB_VERIFIED.add(session_key)
            return accept_source_attestation()
        _record_runtime_build_stage_ms(
            stage_timings_ms,
            "runtime_lib_canonical_hydrate",
            hydrate_start,
        )
        first_build = not runtime_lib.exists()
        if not json_output:
            if first_build:
                print(
                    "Building optimized runtime (first time only)...",
                    file=sys.stderr,
                )
            else:
                print("Runtime sources changed; rebuilding runtime...", file=sys.stderr)
        try:
            with _build_slot() as _slot:
                cargo_build_start = time.perf_counter()
                build = _run_cargo_with_sccache_retry(
                    cmd,
                    cwd=project_root,
                    env=build_env,
                    timeout=cargo_timeout,
                    json_output=json_output,
                    label="Runtime build",
                )
                _record_runtime_build_stage_ms(
                    stage_timings_ms,
                    "runtime_lib_cargo_build",
                    cargo_build_start,
                )
        except subprocess.TimeoutExpired:
            if not json_output:
                timeout_note = (
                    f"Runtime build timed out after {cargo_timeout:.1f}s."
                    if cargo_timeout is not None
                    else "Runtime build timed out."
                )
                print(timeout_note, file=sys.stderr)
            return False
        if build.returncode != 0:
            err = build.stderr.strip() or build.stdout.strip()
            if err:
                print(err, file=sys.stderr)
            return False
        cargo_runtime_lib = _runtime_cargo_scratch_lib_path(runtime_lib, target_triple)
        if cargo_runtime_lib != runtime_lib:
            if not cargo_runtime_lib.exists():
                if not json_output:
                    print(
                        f"Runtime build succeeded but archive is missing: {cargo_runtime_lib}",
                        file=sys.stderr,
                    )
                return False
            try:
                _atomic_copy_file(cargo_runtime_lib, runtime_lib)
            except OSError as exc:
                if not json_output:
                    print(
                        f"Failed to materialize runtime archive alias {runtime_lib}: {exc}",
                        file=sys.stderr,
                    )
                return False
        try:
            write_native_link_dependency_manifest(
                build.stdout,
                cargo_stderr=build.stderr,
                runtime_lib=runtime_lib,
                cargo_profile=cargo_profile,
                target_triple=target_triple,
                source_root=project_root,
                source_fingerprint=source_fingerprint,
            )
        except (OSError, NativeLinkDependencyManifestError) as exc:
            if not json_output:
                print(
                    f"Failed to publish runtime native-link manifest: {exc}",
                    file=sys.stderr,
                )
            return False
        if fingerprint is not None:
            try:
                fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(
                    fingerprint_path,
                    fingerprint,
                    artifact=runtime_lib,
                )
            except OSError:
                if not json_output:
                    print(
                        "Warning: failed to write runtime fingerprint metadata.",
                        file=sys.stderr,
                    )
        if session_key is not None:
            _RUNTIME_LIB_VERIFIED.add(session_key)
    return accept_source_attestation()


def _runtime_build_profile_override() -> str:
    """Opt-in iteration-loop override for the runtime-wasm cargo profile.

    ``MOLT_RUNTIME_BUILD_PROFILE`` (e.g. ``dev-fast``) swaps the runtime-wasm
    cargo profile so a correctness-iteration loop (the E1 witness numpy-import
    debug loop) does not pay full ``release-output`` (fat-LTO, opt-``z``) codegen
    on every invalidated rebuild â€” opt level does not change the deterministic
    import outcome it is chasing.  DEFAULT UNCHANGED: when the knob is unset,
    acceptance / final-green still builds the shipped ``release-output`` runtime,
    which is the artifact parity is measured against (M05).  An invalid profile
    name is ignored so a typo cannot silently redirect the build; cargo would
    also reject a non-existent profile loudly.
    """
    raw = os.environ.get("MOLT_RUNTIME_BUILD_PROFILE", "").strip()
    if raw and _CARGO_PROFILE_NAME_RE.match(raw):
        return raw
    return ""


@functools.lru_cache(maxsize=32)
def _resolve_wasm_cargo_profile_cached(
    cargo_profile: str,
    override: str,
    runtime_build_profile: str,
) -> str:
    # Precedence: the explicit MOLT_WASM_CARGO_PROFILE override (pre-existing
    # contract) wins, then the MOLT_RUNTIME_BUILD_PROFILE iteration knob, then
    # the derived default. Keeping the iteration knob below the explicit
    # override means an operator who pinned MOLT_WASM_CARGO_PROFILE is never
    # surprised by it.
    if override:
        return override
    if runtime_build_profile:
        return runtime_build_profile
    if cargo_profile == "release":
        return "wasm-release"
    return cargo_profile


def _resolve_wasm_cargo_profile(cargo_profile: str) -> str:
    """Map cargo profile for WASM targets.

    Uses the explicit ``wasm-release`` profile instead of generic ``release``
    so WASM artifact size/perf policy can move independently from native
    staticlib policy. Override with ``MOLT_WASM_CARGO_PROFILE`` (explicit) or the
    iteration-scoped ``MOLT_RUNTIME_BUILD_PROFILE`` (e.g. ``dev-fast``).
    """
    return _resolve_wasm_cargo_profile_cached(
        cargo_profile,
        os.environ.get("MOLT_WASM_CARGO_PROFILE", "").strip(),
        _runtime_build_profile_override(),
    )


def _ensure_runtime_wasm_artifact(
    runtime_state: _RuntimeArtifactState,
    *,
    reloc: bool,
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
) -> bool:
    runtime_path = (
        runtime_state.runtime_reloc_wasm if reloc else runtime_state.runtime_wasm
    )
    requested_exports = (
        None if required_exports is None else frozenset(required_exports)
    )
    requested_features = frozenset(required_link_features)
    ready_export_sets = (
        runtime_state.runtime_reloc_wasm_ready_export_sets
        if reloc
        else runtime_state.runtime_wasm_ready_export_sets
    )
    ready_feature_keys = (
        runtime_state.runtime_reloc_wasm_ready_feature_keys
        if reloc
        else runtime_state.runtime_wasm_ready_feature_keys
    )
    ready_key = (requested_features, requested_exports)
    ready_all_exports_key = (requested_features, None)
    ready = (
        runtime_state.runtime_reloc_wasm_ready
        if reloc
        else runtime_state.runtime_wasm_ready
    )
    if runtime_path is None:
        return True
    if ready_key in ready_feature_keys or ready_all_exports_key in ready_feature_keys:
        return True
    if not requested_features and (
        None in ready_export_sets or requested_exports in ready_export_sets
    ):
        return True
    if ready and required_exports is None and not requested_features:
        ready_export_sets.add(None)
        ready_feature_keys.add(ready_key)
        return True
    if not _ensure_runtime_wasm(
        runtime_path,
        reloc=reloc,
        json_output=json_output,
        cargo_profile=cargo_profile,
        cargo_timeout=cargo_timeout,
        project_root=project_root,
        simd_enabled=simd_enabled,
        freestanding=freestanding,
        stdlib_profile=stdlib_profile,
        resolved_modules=resolved_modules,
        required_link_features=required_link_features,
        required_exports=required_exports,
    ):
        return False
    if reloc:
        runtime_state.runtime_reloc_wasm_ready = True
    else:
        runtime_state.runtime_wasm_ready = True
    ready_export_sets.add(requested_exports)
    ready_feature_keys.add(ready_key)
    return True


def _prebuild_runtime_wasm(
    *,
    project_root: Path,
    kind: Literal["shared", "reloc", "both"],
    json_output: bool,
    build_profile: BuildProfile,
    cargo_timeout: float | None,
    simd_enabled: bool = True,
    freestanding: bool = False,
    stdlib_profile: str | None = DEFAULT_STDLIB_PROFILE,
    verbose: bool = False,
) -> int:
    cargo_profile, profile_error = _resolve_cargo_profile_name(build_profile)
    if profile_error is not None:
        if json_output:
            print(json.dumps({"ok": False, "error": profile_error}))
        else:
            print(profile_error, file=sys.stderr)
        return 1
    concrete_stdlib_profile = runtime_stdlib_profile_for_required_features(
        stdlib_profile,
        frozenset(),
        target_triple="wasm32-wasip1",
    )
    runtime_state = _initialize_runtime_artifact_state(
        is_rust_transpile=False,
        is_wasm=True,
        emit_mode="wasm",
        molt_root=project_root,
        runtime_cargo_profile=cargo_profile,
        target_triple=None,
        stdlib_profile=concrete_stdlib_profile,
    )
    artifacts: dict[str, str] = {}
    if kind == "both":
        # V1 dual-compile burn-down: a single combined `cargo rustc --lib`
        # (no --crate-type override) emits both the staticlib and cdylib, so the
        # reloc and shared artifacts are produced from ONE compile instead of two.
        if (
            runtime_state.runtime_wasm is None
            or runtime_state.runtime_reloc_wasm is None
        ):
            if not json_output:
                print(
                    "Runtime wasm shared/reloc artifact path is unavailable.",
                    file=sys.stderr,
                )
            return 1
        if verbose and not json_output:
            print(
                "Prebuilding runtime wasm shared+reloc (single combined compile): "
                f"{runtime_state.runtime_wasm}",
                file=sys.stderr,
            )
        if not _ensure_runtime_wasm_both(
            runtime_state,
            json_output=json_output,
            cargo_profile=cargo_profile,
            cargo_timeout=cargo_timeout,
            project_root=project_root,
            simd_enabled=simd_enabled,
            freestanding=freestanding,
            stdlib_profile=concrete_stdlib_profile,
            resolved_modules=None,
            required_exports=None,
        ):
            if not json_output:
                print("Runtime wasm prebuild failed.", file=sys.stderr)
            return 1
        artifacts["shared"] = os.fspath(runtime_state.runtime_wasm)
        artifacts["reloc"] = os.fspath(runtime_state.runtime_reloc_wasm)
    else:
        label = "shared" if kind == "shared" else "reloc"
        reloc = kind == "reloc"
        runtime_path = (
            runtime_state.runtime_reloc_wasm if reloc else runtime_state.runtime_wasm
        )
        if runtime_path is None:
            if not json_output:
                print(
                    f"Runtime wasm {label} artifact path is unavailable.",
                    file=sys.stderr,
                )
            return 1
        if verbose and not json_output:
            print(
                f"Prebuilding runtime wasm {label} artifact: {runtime_path}",
                file=sys.stderr,
            )
        if not _ensure_runtime_wasm_artifact(
            runtime_state,
            reloc=reloc,
            json_output=json_output,
            cargo_profile=cargo_profile,
            cargo_timeout=cargo_timeout,
            project_root=project_root,
            simd_enabled=simd_enabled,
            freestanding=freestanding,
            stdlib_profile=concrete_stdlib_profile,
            resolved_modules=None,
            required_exports=None,
        ):
            if not json_output:
                print(f"Runtime wasm {label} prebuild failed.", file=sys.stderr)
            return 1
        artifacts[label] = os.fspath(runtime_path)
    _emit_runtime_wasm_build_timings(json_output=json_output)
    if json_output:
        print(
            json.dumps(
                {"status": "ok", "artifacts": artifacts},
                sort_keys=True,
            )
        )
    elif verbose:
        for label, path in artifacts.items():
            print(f"Runtime wasm {label}: {path}", file=sys.stderr)
    return 0


def _emit_runtime_wasm_build_timings(*, json_output: bool) -> None:
    """Emit the per-phase runtime-wasm build timings under MOLT_BUILD_DIAGNOSTICS.

    Extends the task #21 diagnostics surface to the standalone
    ``internal-runtime-wasm-build`` entry (which does not assemble the full
    build-diagnostics payload).  ``MOLT_BUILD_DIAGNOSTICS`` gates a compact,
    machine-readable JSON line on stderr and an optional JSON file at
    ``MOLT_BUILD_DIAGNOSTICS_FILE`` so the ``--kind both`` single-compile
    acceptance is a measured number (doctrine 74 law 4).
    """
    if os.environ.get("MOLT_BUILD_DIAGNOSTICS", "").strip().lower() not in {
        "1",
        "true",
        "yes",
        "on",
    }:
        return
    snapshot = _runtime_wasm_build_timings_snapshot()
    if snapshot is None:
        return
    line = json.dumps({"runtime_wasm_build": snapshot}, sort_keys=True)
    print(f"MOLT_BUILD_DIAGNOSTICS runtime_wasm_build: {line}", file=sys.stderr)
    out_spec = os.environ.get("MOLT_BUILD_DIAGNOSTICS_FILE", "").strip()
    if out_spec:
        with contextlib.suppress(OSError):
            _atomic_write_text(Path(out_spec), line + "\n")


def _configure_wasm_cc_env(env: dict[str, str]) -> None:
    if env.get("CC_wasm32-wasip1") or env.get("CC_wasm32_wasip1"):
        return
    for candidate in (
        "/opt/homebrew/opt/llvm/bin/clang",
        "/usr/local/opt/llvm/bin/clang",
    ):
        cc_path = Path(candidate)
        if cc_path.exists() and os.access(cc_path, os.X_OK):
            env["CC_wasm32-wasip1"] = str(cc_path)
            env["CC_wasm32_wasip1"] = str(cc_path)
            return


def _configure_wasi_sysroot_env(env: dict[str, str]) -> None:
    explicit_sysroot = env.get("WASI_SYSROOT") or env.get("MOLT_WASI_SYSROOT")
    if explicit_sysroot:
        normalized = wasm_toolchain.normalize_wasi_sysroot(explicit_sysroot)
        sysroot = str(normalized if normalized is not None else Path(explicit_sysroot))
        env.setdefault("WASI_SYSROOT", sysroot)
        env.setdefault("MOLT_WASI_SYSROOT", sysroot)
        return
    wasi_sysroot = wasm_toolchain.resolve_wasi_sysroot()
    if wasi_sysroot is not None:
        sysroot = str(wasi_sysroot)
        env["WASI_SYSROOT"] = sysroot
        env["MOLT_WASI_SYSROOT"] = sysroot


def _configure_wasm_long_double_env(env: dict[str, str]) -> None:
    """Thread the resolved long-double link archives to molt-runtime's build.rs.

    The deploy ``molt_runtime.wasm`` cdylib link is rustc-driven (so molt cannot
    order a trailing ``-lc-printscan-long-double`` ahead of the self-contained
    ``-lc``); build.rs instead links these archives as build-script
    ``rustc-link-lib`` entries, which rustc emits in its LOCAL-native-libraries
    group AHEAD of ``-lc`` â€” the real ``vfprintf``/``__floatscan`` override
    wasi-libc's ``long_double_not_supported`` stub. This is the deploy-cdylib arm
    of the SAME single authority the reloc / split-app ``wasm-ld`` paths apply;
    env-threaded so build.rs consumes the Python resolver's path (incl. the
    durable ``vendor/wasm-builtins`` fallback), not merely a session sysroot. The
    ``artifact_poison_gate`` attests the effect on the built cdylib. (Harmless on
    the sibling staticlib crate-type: ``rustc-link-lib`` is metadata there, and
    the reloc link whole-archives its own printscan copy.)
    """
    policy = wasm_toolchain.resolve_long_double_link_policy(required=False)
    if policy.printscan is not None:
        env["MOLT_WASM_LONGDOUBLE_ARCHIVE"] = str(
            policy.printscan.resolve(strict=False)
        )
    if policy.builtins is not None:
        env["MOLT_WASM_BUILTINS_ARCHIVE"] = str(policy.builtins.resolve(strict=False))


def _wasm_runtime_artifact_path(target_root: Path, profile_dir: str) -> Path:
    return target_root / "wasm32-wasip1" / profile_dir / "molt_runtime.wasm"


def _wasm_runtime_staticlib_path(target_root: Path, profile_dir: str) -> Path:
    return target_root / "wasm32-wasip1" / profile_dir / "libmolt_runtime.a"


def _wasm_cpython_abi_staticlib_path(target_root: Path, profile_dir: str) -> Path:
    return target_root / "wasm32-wasip1" / profile_dir / "libmolt_cpython_abi.a"


def _wasm_cpython_abi_staticlib_candidates(
    target_root: Path,
    profile_dir: str,
) -> list[Path]:
    primary = _wasm_cpython_abi_staticlib_path(target_root, profile_dir)
    candidates: list[Path] = []
    if primary.exists():
        candidates.append(primary)
    deps_dir = _wasm_runtime_deps_dir(target_root, profile_dir)
    deps_primary = deps_dir / "libmolt_cpython_abi.a"
    if deps_primary.exists():
        candidates.append(deps_primary)
    deps_candidates: list[tuple[int, str, Path]] = []
    for path in deps_dir.glob("libmolt_cpython_abi-*.a"):
        try:
            stat = path.stat()
        except OSError:
            continue
        deps_candidates.append((stat.st_mtime_ns, path.name, path))
    candidates.extend(
        path for _mtime_ns, _name, path in sorted(deps_candidates, reverse=True)
    )
    return candidates


def _resolve_built_runtime_staticlib_artifact(
    target_root: Path, profile_dir: str
) -> Path:
    candidates = _wasm_runtime_staticlib_candidates(target_root, profile_dir)
    if candidates:
        return candidates[0]
    return _wasm_runtime_staticlib_path(target_root, profile_dir)


def _wasm_runtime_staticlib_candidates(
    target_root: Path,
    profile_dir: str,
) -> list[Path]:
    primary = _wasm_runtime_staticlib_path(target_root, profile_dir)
    candidates: list[Path] = []
    if primary.exists():
        candidates.append(primary)
    deps_dir = _wasm_runtime_deps_dir(target_root, profile_dir)
    deps_candidates: list[tuple[int, str, Path]] = []
    for path in deps_dir.glob("libmolt_runtime-*.a"):
        try:
            stat = path.stat()
        except OSError:
            continue
        deps_candidates.append((stat.st_mtime_ns, path.name, path))
    candidates.extend(
        path for _mtime_ns, _name, path in sorted(deps_candidates, reverse=True)
    )
    return candidates


def _wasm_runtime_deps_dir(target_root: Path, profile_dir: str) -> Path:
    return target_root / "wasm32-wasip1" / profile_dir / "deps"


def _ensure_wasm_cpython_abi_staticlib(
    *,
    project_root: Path,
    json_output: bool,
    cargo_profile: str,
    cargo_timeout: float | None,
) -> Path | None:
    root = project_root or _compiler_root()
    cargo_profile = _resolve_wasm_cargo_profile(cargo_profile)
    profile_dir = _cargo_profile_dir(cargo_profile)
    target_root = _cargo_target_root(root)
    staticlib_path = _wasm_cpython_abi_staticlib_path(target_root, profile_dir)
    target_label = "wasm32-wasip1.cpython-abi"
    fingerprint_path = _runtime_fingerprint_path(
        root,
        staticlib_path,
        cargo_profile,
        target_label,
    )
    base_rustflags = os.environ.get("RUSTFLAGS", "").strip()
    rustflags = _wasm_runtime_codegen_rustflags(
        base_rustflags,
        simd_enabled=True,
        freestanding=False,
    )
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    fingerprint = _runtime_fingerprint(
        root,
        cargo_profile=cargo_profile,
        target_triple="wasm32-wasip1",
        rustflags=rustflags,
        runtime_features=("molt-cpython-abi-static-link",),
        stored_fingerprint=stored_fingerprint,
    )
    candidates = _wasm_cpython_abi_staticlib_candidates(target_root, profile_dir)
    if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") == "1":
        for candidate in candidates:
            if candidate.exists():
                return candidate
    if fingerprint is None:
        if not json_output:
            print("Failed to compute CPython ABI wasm fingerprint.", file=sys.stderr)
        return None

    lock_name = f"runtime.{cargo_profile}.wasm32-wasip1.cpython-abi"
    build_state_root = _build_state_root(root)
    with _build_lock(root, lock_name):
        current = _current_runtime_target_artifact(
            _wasm_cpython_abi_staticlib_candidates(target_root, profile_dir),
            build_state_root=build_state_root,
            cargo_profile=cargo_profile,
            target_label=target_label,
            fingerprint=fingerprint,
        )
        if current is not None:
            return current[0]
        if _runtime_artifact_fingerprint_matches(
            staticlib_path,
            fingerprint,
            fingerprint_path,
            require_artifact_digest=True,
        ):
            if _runtime_fingerprint_metadata_needs_refresh(
                stored_fingerprint,
                fingerprint,
            ):
                with contextlib.suppress(OSError):
                    _refresh_runtime_fingerprint_metadata(
                        fingerprint_path,
                        fingerprint,
                    )
            return staticlib_path

        if not json_output:
            print("Building wasm CPython ABI link provider...", file=sys.stderr)
        env = _cargo_build_env()
        env["CARGO_TARGET_DIR"] = str(target_root)
        if rustflags:
            env["RUSTFLAGS"] = rustflags
        _configure_wasm_cc_env(env)
        _configure_wasi_sysroot_env(env)
        if os.environ.get("MOLT_WASM_DISABLE_SCCACHE") != "1":
            _maybe_enable_sccache(env)
        else:
            env.pop("RUSTC_WRAPPER", None)
        cmd = [
            "cargo",
            "rustc",
            "--package",
            "molt-lang-cpython-abi",
            "--profile",
            cargo_profile,
            "--target",
            "wasm32-wasip1",
            "--lib",
            "--",
            "--crate-type=staticlib",
        ]
        cargo_cmd = _cargo_cmd_with_json_artifact_messages(cmd)
        with _build_slot() as _slot:
            build_raw = _run_subprocess_captured_to_tempfiles(
                cargo_cmd,
                cwd=root,
                env=env,
                timeout=cargo_timeout,
                progress_label=None if json_output else "CPython ABI wasm build",
            )
        build = subprocess.CompletedProcess(
            build_raw.args,
            build_raw.returncode,
            build_raw.stdout.decode("utf-8", errors="replace"),
            build_raw.stderr.decode("utf-8", errors="replace"),
        )
        wrapper = env.get("RUSTC_WRAPPER", "")
        if build.returncode != 0 and wrapper and Path(wrapper).name == "sccache":
            retry_env = env.copy()
            retry_env.pop("RUSTC_WRAPPER", None)
            if not json_output:
                print(
                    "CPython ABI wasm build: sccache wrapper failure detected; "
                    "retrying without sccache.",
                    file=sys.stderr,
                )
            with _build_slot() as _slot:
                build_raw = _run_subprocess_captured_to_tempfiles(
                    cargo_cmd,
                    cwd=root,
                    env=retry_env,
                    timeout=cargo_timeout,
                    progress_label=None if json_output else "CPython ABI wasm build",
                )
            build = subprocess.CompletedProcess(
                build_raw.args,
                build_raw.returncode,
                build_raw.stdout.decode("utf-8", errors="replace"),
                build_raw.stderr.decode("utf-8", errors="replace"),
            )
        if build.returncode != 0:
            detail = (build.stderr or build.stdout or "").strip()
            msg = "CPython ABI wasm build failed"
            if detail:
                msg = f"{msg}: {detail}"
            print(msg, file=sys.stderr)
            return None
        candidates = _wasm_cpython_abi_staticlib_candidates(target_root, profile_dir)
        if not candidates:
            if not json_output:
                print(
                    "CPython ABI wasm build succeeded but staticlib artifact is missing.",
                    file=sys.stderr,
                )
            return None
        provider = candidates[0]
        try:
            fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
            _write_runtime_fingerprint(
                fingerprint_path,
                fingerprint,
                artifact=provider,
            )
            provider_fingerprint_path = _runtime_target_fingerprint_path(
                build_state_root,
                provider,
                cargo_profile=cargo_profile,
                target_label=target_label,
            )
            provider_fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
            _write_runtime_fingerprint(
                provider_fingerprint_path,
                fingerprint,
                artifact=provider,
            )
        except OSError:
            if not json_output:
                print(
                    "Failed to publish CPython ABI wasm staticlib metadata.",
                    file=sys.stderr,
                )
            return None
        return provider


def _resolve_built_runtime_wasm_artifact(target_root: Path, profile_dir: str) -> Path:
    candidates = _wasm_runtime_wasm_candidates(target_root, profile_dir)
    if candidates:
        return candidates[0]
    return _wasm_runtime_artifact_path(target_root, profile_dir)


def _wasm_runtime_wasm_candidates(
    target_root: Path,
    profile_dir: str,
) -> list[Path]:
    primary = _wasm_runtime_artifact_path(target_root, profile_dir)
    candidates: list[Path] = []
    if primary.exists():
        candidates.append(primary)
    deps_primary = (
        _wasm_runtime_deps_dir(target_root, profile_dir) / "molt_runtime.wasm"
    )
    if deps_primary.exists():
        candidates.append(deps_primary)
    deps_dir = _wasm_runtime_deps_dir(target_root, profile_dir)
    deps_candidates: list[tuple[int, str, Path]] = []
    for path in deps_dir.glob("molt_runtime-*.wasm"):
        try:
            stat = path.stat()
        except OSError:
            continue
        deps_candidates.append((stat.st_mtime_ns, path.name, path))
    candidates.extend(
        path for _mtime_ns, _name, path in sorted(deps_candidates, reverse=True)
    )
    return candidates


def _current_runtime_target_artifact(
    candidates: Sequence[Path],
    *,
    build_state_root: Path,
    cargo_profile: str,
    target_label: str,
    fingerprint: dict[str, Any],
) -> tuple[Path, Path] | None:
    for candidate in candidates:
        fingerprint_path = _runtime_target_fingerprint_path(
            build_state_root,
            candidate,
            cargo_profile=cargo_profile,
            target_label=target_label,
        )
        stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
        if _runtime_artifact_fingerprint_matches(
            candidate,
            fingerprint,
            fingerprint_path,
            require_artifact_digest=True,
        ):
            if _runtime_fingerprint_metadata_needs_refresh(
                stored_fingerprint,
                fingerprint,
            ):
                with contextlib.suppress(OSError):
                    _refresh_runtime_fingerprint_metadata(
                        fingerprint_path,
                        fingerprint,
                    )
            return candidate, fingerprint_path
    return None


def _runtime_cargo_report_missing_artifact_path(
    target_root: Path,
    profile_dir: str,
    artifact_kind: Literal["cdylib", "staticlib"],
) -> Path:
    suffix = "a" if artifact_kind == "staticlib" else "wasm"
    return (
        _wasm_runtime_deps_dir(target_root, profile_dir)
        / f".molt_runtime.cargo-report-missing.{suffix}"
    )


def _cargo_cmd_with_json_artifact_messages(cmd: Sequence[str]) -> list[str]:
    if any(arg.startswith("--message-format") for arg in cmd):
        return list(cmd)
    try:
        rustc_arg_index = list(cmd).index("--")
    except ValueError:
        return [*cmd, "--message-format=json-render-diagnostics"]
    return [
        *cmd[:rustc_arg_index],
        "--message-format=json-render-diagnostics",
        *cmd[rustc_arg_index:],
    ]


def _reported_runtime_artifact_matches(
    path: Path,
    *,
    target_root: Path,
    artifact_kind: Literal["cdylib", "staticlib"],
) -> bool:
    try:
        resolved_path = path.resolve(strict=False)
        resolved_root = target_root.resolve(strict=False)
    except OSError:
        return False
    if not (
        resolved_path == resolved_root or resolved_path.is_relative_to(resolved_root)
    ):
        return False
    name = resolved_path.name
    if artifact_kind == "staticlib":
        return name == "libmolt_runtime.a" or (
            name.startswith("libmolt_runtime-") and name.endswith(".a")
        )
    return name == "molt_runtime.wasm" or (
        name.startswith("molt_runtime-") and name.endswith(".wasm")
    )


def _reported_runtime_artifact_from_cargo_stdout(
    stdout: str,
    *,
    target_root: Path,
    artifact_kind: Literal["cdylib", "staticlib"],
) -> Path | None:
    reported: Path | None = None
    for line in stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(message, dict):
            continue
        if message.get("reason") != "compiler-artifact":
            continue
        target = message.get("target")
        target_name = target.get("name") if isinstance(target, dict) else None
        package_id = message.get("package_id")
        package_text = package_id if isinstance(package_id, str) else ""
        if (
            target_name not in {"molt_runtime", "molt-runtime"}
            and "molt-runtime" not in package_text
        ):
            continue
        filenames = message.get("filenames")
        if not isinstance(filenames, list):
            continue
        for filename in filenames:
            if not isinstance(filename, str) or not filename:
                continue
            path = Path(filename)
            if not path.is_absolute():
                path = target_root / path
            if _reported_runtime_artifact_matches(
                path,
                target_root=target_root,
                artifact_kind=artifact_kind,
            ):
                reported = path
    return reported


def _wasm_runtime_recovery_target_root(target_root: Path) -> Path:
    return target_root.parent / f"{target_root.name}-wasm-runtime-recovery"


def _runtime_wasm_incremental_enabled() -> bool:
    """Whether to build the runtime wasm into a stable, incremental target dir.

    The stable per-family target dir (``_runtime_wasm_incremental_target_root``)
    is session-independent, so a fresh session/worktree reuses already-compiled
    dependency crates instead of a cold recompile of the whole graph (V2 cold-dep
    burn-down).  ``CARGO_INCREMENTAL=1`` is turned on with it so consecutive
    same-family iterations recompile incrementally.

    Resolution (V2 doctrine "stable dep-cache default-ON for iteration contexts"):

    * ``MOLT_RUNTIME_WASM_INCREMENTAL`` explicitly set wins, both ways.
    * Otherwise DEFAULT ON in an explicit iteration context -- i.e. when
      ``MOLT_RUNTIME_BUILD_PROFILE`` pins a non-shipping profile (dev-fast) for
      the correctness loop -- so the iteration knob alone enables cross-session
      dep reuse (one knob, progressive disclosure).
    * Otherwise DEFAULT OFF: the shipped acceptance / final-green path
      (which never sets ``MOLT_RUNTIME_BUILD_PROFILE``) keeps the deterministic
      session-scoped target dir and publishes exact-identity artifacts to the
      shared cache (M05); incremental builds deliberately never publish.
    """
    raw = os.environ.get("MOLT_RUNTIME_WASM_INCREMENTAL", "").strip().lower()
    if raw in {"1", "true", "yes", "on"}:
        return True
    if raw in {"0", "false", "no", "off"}:
        return False
    return bool(_runtime_build_profile_override())


def _runtime_wasm_incremental_family_key(
    *,
    cargo_profile: str,
    target_triple: str,
    features: tuple[str, ...],
    simd_enabled: bool,
    freestanding: bool,
) -> str:
    """Codegen-identity key for the incremental runtime-wasm target dir.

    Deliberately EXCLUDES link-args (export allowlist, ``--import-memory`` /
    ``--import-table`` / ``--growable-table``) which are the *only* thing that
    differs between the reloc/staticlib and shared/cdylib passes.  ``molt-runtime``
    declares ``crate-type = ["staticlib", "rlib", "cdylib"]`` so a single rustc
    codegen already emits every crate-type; link-args only re-drive the final
    link.  Keying the shared incremental dir on the codegen family therefore lets
    the second pass reuse the first pass's object code (near-pure re-link) and
    lets consecutive same-config iterations recompile incrementally instead of
    from scratch.
    """
    payload = "\n".join(
        [
            f"profile:{cargo_profile}",
            f"target:{target_triple}",
            f"simd:{int(simd_enabled)}",
            f"freestanding:{int(freestanding)}",
            "features:" + ",".join(sorted(features)),
        ]
    )
    digest = hashlib.sha256(payload.encode("utf-8")).hexdigest()[:16]
    return f"{cargo_profile}-{digest}"


def _runtime_wasm_incremental_target_root(project_root: Path, family_key: str) -> Path:
    """Stable, session-independent cargo target dir for the runtime-wasm build.

    Session-independent by design: cross-iteration incremental reuse is the whole
    point (the per-session dir exists for agent isolation, but a fresh session id
    per proof-queue run means cargo incremental never engages â€” the M09 "stable
    target dir" lever).  Concurrency across sessions building the same family is
    made safe by cargo's own per-target build lock plus the ``_build_slot()``
    cross-process gate; two *divergent* source builds in one family serialise and
    may thrash each other's incremental state (slower, never incorrect), so this
    stays opt-in for the single-lane iteration loop.
    """
    override = os.environ.get("CARGO_TARGET_DIR", "").strip()
    if override:
        base = Path(override).expanduser()
        if not base.is_absolute():
            base = (Path.cwd() / base).absolute()
    else:
        base = project_root / "target"
    return base / "runtime-wasm-incr" / family_key


def _append_rustflags_text(base: str, flags: str) -> str:
    return f"{base.strip()} {flags.strip()}".strip()


def _wasm_runtime_codegen_rustflags(
    rustflags: str,
    *,
    simd_enabled: bool,
    freestanding: bool,
) -> str:
    # Disable reference-types so that LLVM (Rust 1.94+ / LLVM 21+) does not
    # emit GC-proposal rec groups or `exact` heap types.  These are rejected
    # by Cloudflare Workers' V8 and by wasm-opt without --all-features.
    # Enable WASM SIMD (128-bit) for vectorized string/bytes operations.
    # Freestanding builds use the conservative baseline because the WASI stub
    # rewriter currently cannot remap SIMD-prefixed instruction streams.
    if "-C target-feature" not in rustflags:
        tf_parts = ["-reference-types"]
        if simd_enabled:
            tf_parts.append("+simd128")
        rustflags = _append_rustflags_text(
            rustflags, f"-C target-feature={','.join(tf_parts)}"
        )
    elif "-reference-types" not in rustflags:
        # Caller already set -C target-feature; append the ref-types disable.
        rustflags = rustflags.replace(
            "-C target-feature=", "-C target-feature=-reference-types,", 1
        )
    if freestanding and 'getrandom_backend="' not in rustflags:
        rustflags = _append_rustflags_text(
            rustflags, '--cfg getrandom_backend="unsupported"'
        )
    return rustflags


def _run_runtime_wasm_cargo_build(
    *,
    cmd: list[str],
    root: Path,
    env: dict[str, str],
    cargo_timeout: float | None,
    profile_dir: str,
    target_root_override: Path | None = None,
    json_output: bool,
    artifact_kind: Literal["cdylib", "staticlib"] = "cdylib",
) -> tuple[subprocess.CompletedProcess[str], Path]:
    build_env = env.copy()
    if target_root_override is not None:
        target_root = target_root_override
    else:
        target_root = _cargo_target_root(root)
    # Always propagate target_root to CARGO_TARGET_DIR so cargo builds
    # into the same directory the artifact lookup will check. Without
    # this, explicit and session-aware target resolution can drift apart.
    build_env["CARGO_TARGET_DIR"] = str(target_root)
    cargo_cmd = _cargo_cmd_with_json_artifact_messages(cmd)
    with _build_slot() as _slot:
        build_raw = _run_subprocess_captured_to_tempfiles(
            cargo_cmd,
            cwd=root,
            env=build_env,
            timeout=cargo_timeout,
            progress_label=None if json_output else "Runtime wasm build",
        )
    build = subprocess.CompletedProcess(
        build_raw.args,
        build_raw.returncode,
        build_raw.stdout.decode("utf-8", errors="replace"),
        build_raw.stderr.decode("utf-8", errors="replace"),
    )
    wrapper = build_env.get("RUSTC_WRAPPER", "")
    if build.returncode != 0 and wrapper and Path(wrapper).name == "sccache":
        retry_env = build_env.copy()
        retry_env.pop("RUSTC_WRAPPER", None)
        if not json_output:
            print(
                "Runtime wasm build: sccache wrapper failure detected; retrying without sccache.",
                file=sys.stderr,
            )
        with _build_slot() as _slot:
            build_raw = _run_subprocess_captured_to_tempfiles(
                cargo_cmd,
                cwd=root,
                env=retry_env,
                timeout=cargo_timeout,
                progress_label=None if json_output else "Runtime wasm build",
            )
        build = subprocess.CompletedProcess(
            build_raw.args,
            build_raw.returncode,
            build_raw.stdout.decode("utf-8", errors="replace"),
            build_raw.stderr.decode("utf-8", errors="replace"),
        )
    reported_artifact = _reported_runtime_artifact_from_cargo_stdout(
        build.stdout,
        target_root=target_root,
        artifact_kind=artifact_kind,
    )
    if reported_artifact is None:
        reported_artifact = _runtime_cargo_report_missing_artifact_path(
            target_root,
            profile_dir,
            artifact_kind,
        )
    return build, reported_artifact


def _reloc_link_archive_fingerprint_token() -> str:
    """Content token for the reloc link's long-double + builtins archives.

    Folded into the reloc-runtime-wasm fingerprint so a change to those archives
    (first provisioning, a version bump, or removal) invalidates the cached
    reloc runtime. Uses (name, size, mtime) â€” cheap and sufficient to detect a
    swapped/updated archive without hashing hundreds of KB every build.
    """
    parts: list[str] = []
    for label, archive in (
        ("longdouble", wasm_toolchain.wasm_wasi_printscan_long_double_archive()),
        ("builtins", wasm_toolchain.wasm_clang_rt_builtins_archive()),
    ):
        if archive is None:
            parts.append(f"{label}=none")
            continue
        try:
            st = archive.stat()
            parts.append(f"{label}={archive.name}:{st.st_size}:{int(st.st_mtime)}")
        except OSError:
            parts.append(f"{label}={archive.name}:unstat")
    return hashlib.sha256(";".join(parts).encode("utf-8")).hexdigest()[:16]


# Top-level packages whose native extensions format/parse `long double` (%L)
# during import, so the reloc runtime they link against MUST carry wasi-libc's
# long-double formatters (else the stub abort()s -> unreachable at import).
_LONG_DOUBLE_MODULE_PREFIXES = frozenset({"numpy", "scipy"})


def _reloc_runtime_requires_long_double(
    *,
    resolved_modules: set[str] | frozenset[str] | None,
    required_exports: set[str] | frozenset[str] | None,
) -> bool:
    """Whether this reloc runtime links code that hits wasi-libc's ``%L`` path.

    True for the CPython-ABI tier (numpy/scipy C extensions format/parse
    ``long double`` during import) â€” identified by a non-empty CPython-ABI
    requested-export set â€” or when a resolved module is (a submodule of) numpy or
    scipy. For these builds a missing long-double formatter archive is a HARD
    ERROR (the runtime would relink wasi-libc's ``long_double_not_supported``
    abort() stub -> raw ``unreachable`` trap at ``_multiarray_umath`` import), not
    a silent graceful degrade. Non-numpy / micro builds stay degradable.
    """
    if wasm_cpython_abi_requested_export_names(required_exports):
        return True
    if resolved_modules:
        for module in resolved_modules:
            if module.split(".", 1)[0] in _LONG_DOUBLE_MODULE_PREFIXES:
                return True
    return False


class _RelocLongDoubleArchives(NamedTuple):
    """Resolved reloc long-double link archives + fail-loud / degrade decision."""

    longdouble: Path | None
    builtins: Path | None
    error: str | None
    warnings: tuple[str, ...]


def _resolve_reloc_long_double_archives(
    *, long_double_required: bool
) -> _RelocLongDoubleArchives:
    """Resolve the reloc long-double archives and decide fail-loud vs degrade.

    Thin wrapper over the single authority
    :func:`wasm_toolchain.resolve_long_double_link_policy` that additionally
    records the ``longdouble_archives`` build attestation (present/MISSING). When
    ``long_double_required`` and either archive is unresolved, propagates the
    authority's ``error`` (the caller MUST abort the build â€” a numpy runtime that
    traps is never acceptable). Otherwise returns the archives plus any degrade
    ``warnings`` for a build that provably does not need long double.
    """
    policy = wasm_toolchain.resolve_long_double_link_policy(
        required=long_double_required
    )
    missing = policy.printscan is None or policy.builtins is None
    if not long_double_required:
        _record_runtime_wasm_longdouble_archives(
            "not_required" if missing else "present"
        )
    else:
        _record_runtime_wasm_longdouble_archives("MISSING" if missing else "present")
    return _RelocLongDoubleArchives(
        policy.printscan, policy.builtins, policy.error, policy.warnings
    )


def _link_runtime_staticlib_to_reloc_wasm(
    *,
    staticlib_path: Path,
    output_path: Path,
    json_output: bool,
    link_timeout: float | None,
    export_link_args: str = "",
    long_double_required: bool = False,
) -> bool:
    wasm_ld = shutil.which("wasm-ld")
    if wasm_ld is None:
        if not json_output:
            print(
                "Runtime relocatable wasm link failed: wasm-ld not found.",
                file=sys.stderr,
            )
        return False
    libc_archive = wasm_toolchain.wasm_wasi_libc_archive()
    if libc_archive is None:
        if not json_output:
            print(
                "Runtime relocatable wasm link failed: Rust wasm32-wasip1 libc.a not found.",
                file=sys.stderr,
            )
        return False
    staticlib_path = staticlib_path.resolve(strict=False)
    libc_archive = libc_archive.resolve(strict=False)
    output_path = output_path.resolve(strict=False)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    tmp_output_path = output_path.with_name(
        f".{output_path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp"
    )
    # E1 witness fix (long-double %L trap): wasi-libc's default libc.a stubs the
    # `%L` (long double) printf/scanf conversions with a `long_double_not_supported`
    # abort() that lowers to a raw `unreachable` trap â€” reached by numpy's
    # longdouble repr/parse (NumPyOS_ascii_formatl/strtold) during
    # `_multiarray_umath` import. Whole-archive wasi-libc's companion long-double
    # formatter archive so its real vfprintf/vfscanf/strtod/floatscan override the
    # stub objects (libc.a's stay lazy and are skipped once defined), and add
    # wasi-sdk's compiler-rt builtins so the binary128 soft-float the formatters
    # call (__addtf3/__multf3/â€¦) â€” and numpy's own longdouble arithmetic â€” resolve
    # here instead of degrading to unresolved imports at the final app link.
    #
    # When this runtime links numpy/scipy (``long_double_required``), a missing
    # archive is a HARD ERROR: building a runtime that would relink the abort()
    # stub and trap at import is never acceptable, and the old warn-but-proceed
    # graceful degrade silently masked exactly that regression. Non-numpy / micro
    # builds keep the degrade path (they never hit %L).
    archives = _resolve_reloc_long_double_archives(
        long_double_required=long_double_required
    )
    if archives.error is not None:
        if not json_output:
            print(archives.error, file=sys.stderr)
        return False
    if not json_output:
        for warning in archives.warnings:
            print(warning, file=sys.stderr)
    # Single authority (reloc arm): whole-archive the staticlib + printscan's
    # real long-double formatters ahead of libc.a; libc.a + builtins stay lazy.
    long_double_argv = wasm_toolchain.long_double_whole_archive_link_argv(
        wasm_toolchain.LongDoubleLinkPolicy(
            archives.longdouble, archives.builtins, archives.error, archives.warnings
        ),
        whole_archive=[str(staticlib_path)],
        trailing=[str(libc_archive)],
    )
    export_args = _wasm_link_args_from_rustflags(export_link_args)
    if export_args:
        export_response_path = _write_wasm_link_args_response_file(
            output_path.parent / ".molt_link_args",
            label=f"{output_path.stem}.reloc",
            link_args=export_args,
        )
        export_args = [f"@{export_response_path}"]
    try:
        process = _run_completed_command(
            [
                wasm_ld,
                "-r",
                *export_args,
                *long_double_argv,
                "-o",
                str(tmp_output_path),
            ],
            cwd=output_path.parent,
            env=None,
            capture_output=True,
            memory_guard_prefix="MOLT_WASM_LINK",
            timeout=link_timeout,
        )
        if process.returncode != 0:
            if not json_output:
                err = (process.stderr or "").strip() or (process.stdout or "").strip()
                msg = "Runtime relocatable wasm link failed"
                if err:
                    msg = f"{msg}: {err}"
                print(msg, file=sys.stderr)
            return False
        if not _is_valid_runtime_wasm_artifact(tmp_output_path):
            if not json_output:
                print(
                    f"Runtime relocatable wasm artifact is invalid/incomplete: {tmp_output_path}",
                    file=sys.stderr,
                )
            return False
        os.replace(tmp_output_path, output_path)
        if os.name == "posix":
            with contextlib.suppress(OSError):
                dir_fd = os.open(output_path.parent, os.O_RDONLY)
                try:
                    os.fsync(dir_fd)
                finally:
                    os.close(dir_fd)
    finally:
        with contextlib.suppress(OSError):
            if tmp_output_path.exists():
                tmp_output_path.unlink()
    return True


def _materialize_split_runtime_public_exports(
    runtime_wasm: Path,
    required_exports: set[str] | frozenset[str] | None,
    *,
    json_output: bool,
) -> bool:
    rename_map = wasm_split_runtime_export_rename_map(required_exports)
    if not rename_map:
        return True
    try:
        updated = rename_wasm_export_names(runtime_wasm.read_bytes(), rename_map)
        if updated is not None:
            _atomic_write_bytes(runtime_wasm, updated)
    except (OSError, ValueError) as exc:
        if not json_output:
            print(
                f"Failed to materialize split-runtime public exports: {exc}",
                file=sys.stderr,
            )
        return False
    return True


def _runtime_exports_satisfy_for_mode(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
    *,
    reloc: bool,
) -> bool:
    if reloc:
        return _runtime_wasm_exports_satisfy(path, required_exports)
    return _split_runtime_wasm_exports_satisfy(path, required_exports)


def _runtime_missing_exports_for_mode(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
    *,
    reloc: bool,
) -> set[str]:
    if reloc:
        return _runtime_wasm_missing_exports(path, required_exports)
    return _split_runtime_wasm_missing_exports(path, required_exports)


class _RuntimeWasmBuildSpec(NamedTuple):
    """Resolved, mode-specific build spec for one runtime-wasm artifact.

    Single source of truth for the cargo profile, feature plan, RUSTFLAGS, and
    the content-address ``fingerprint`` of a reloc/shared runtime-wasm build.
    Shared by ``_ensure_runtime_wasm`` (which consumes exactly these values) and
    ``_prepopulate_combined_runtime_wasm_target`` (which must compute a
    byte-identical ``fingerprint`` so the combined single-compile's artifacts are
    recognised by the per-artifact target-reuse fast path).  Keeping one
    authority means the two can never silently drift out of fingerprint parity.
    """

    requested_cargo_profile: str
    cargo_profile: str
    profile_dir: str
    incremental_enabled: bool
    env: dict[str, str]
    runtime_exports: str
    link_flags: str
    cargo_link_response_path: Path | None
    cargo_rustflags: str
    fingerprint_rustflags: str
    no_default_features: bool
    wasm_cargo_features: tuple[str, ...]
    fingerprint_features: tuple[str, ...]
    fingerprint_path: Path
    target_root: Path
    stored_fingerprint: dict[str, Any] | None
    fingerprint: dict[str, Any] | None


def _compute_runtime_wasm_build_spec(
    root: Path,
    runtime_wasm: Path,
    *,
    reloc: bool,
    cargo_profile: str,
    simd_enabled: bool,
    freestanding: bool,
    stdlib_profile: str | None,
    resolved_modules: set[str] | frozenset[str] | None,
    required_link_features: frozenset[str],
    required_exports: set[str] | frozenset[str] | None,
) -> _RuntimeWasmBuildSpec:
    """Resolve the mode-specific runtime-wasm build spec (see _RuntimeWasmBuildSpec)."""
    # The emitted app import ABI is the final link-time requirement authority.
    # Reachability-derived features normally predict this set, but external
    # native objects and runtime-support module initializers can add imports
    # after that earlier scan.  Project every required export through the same
    # generated symbol->feature authority and close the feature plan here, so
    # Cargo can never build an artifact that the immediately following export
    # validator proves insufficient.
    export_link_features = frozenset(
        feature
        for symbol in required_exports or ()
        if (feature := link_affecting_feature_gate_for_symbol(symbol)) is not None
    )
    required_link_features = frozenset(required_link_features) | export_link_features
    requested_cargo_profile = cargo_profile
    cargo_profile = _resolve_wasm_cargo_profile(cargo_profile)
    profile_dir = _cargo_profile_dir(cargo_profile)
    incremental_enabled = _runtime_wasm_incremental_enabled()
    env = _cargo_build_env()
    if "CARGO_INCREMENTAL" not in os.environ:
        env["CARGO_INCREMENTAL"] = "1" if incremental_enabled else "0"
    cpython_abi_requested_exports = wasm_cpython_abi_requested_export_names(
        required_exports
    )
    if cpython_abi_requested_exports:
        env["MOLT_WASM_CPYTHON_ABI_EXPORTS"] = "\n".join(cpython_abi_requested_exports)
        cpython_abi_requested_data_exports = (
            wasm_cpython_abi_requested_data_export_names(required_exports)
        )
        if cpython_abi_requested_data_exports:
            env["MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS"] = "\n".join(
                cpython_abi_requested_data_exports
            )
    if reloc:
        runtime_exports = wasm_runtime_export_link_args(
            required_exports,
            resolved_modules=resolved_modules,
        )
        link_flags = runtime_exports
        cargo_link_response_path = None
    else:
        runtime_exports = wasm_runtime_shared_export_link_args(required_exports)
        shared_import_flags = (
            "-C link-arg=--import-memory -C link-arg=--import-table"
            " -C link-arg=--growable-table"
        )
        link_flags = f"{shared_import_flags}{runtime_exports}"
        cargo_link_response_path = _wasm_link_args_response_file(
            root,
            label=f"runtime.{_resolve_wasm_cargo_profile(cargo_profile)}.shared",
            link_flags=link_flags,
        )
    base_rustflags = env.get("RUSTFLAGS", "").strip()
    cargo_rustflags = _wasm_runtime_codegen_rustflags(
        base_rustflags,
        simd_enabled=simd_enabled,
        freestanding=freestanding,
    )
    fingerprint_rustflags = _wasm_runtime_codegen_rustflags(
        _append_rustflags_text(base_rustflags, link_flags),
        simd_enabled=simd_enabled,
        freestanding=freestanding,
    )
    # Fold the long-double archive identity into BOTH runtime fingerprints so a
    # change to those archives (first provisioning, version bump, or removal)
    # invalidates the cached runtime instead of serving a stale
    # long-double-stubbed one (effect-attestation: configured != effective, M34).
    # The reloc link whole-archives them via wasm-ld; the shared cdylib links
    # them via build.rs `rustc-link-lib` (see _configure_wasm_long_double_env) â€”
    # in BOTH cases the archives never otherwise enter the fingerprint/compat
    # digest (the shared build passes them by env, not rustc link-args). The tag
    # is a pure fingerprint input (a distinct cfg name per crate-type, never
    # handed to the real compile) so the emitted wasm stays byte-identical for a
    # fixed archive set â€” the CDN-cacheable shared runtime is stable across
    # builds and only re-keys when the archives actually change.
    _longdouble_link_token = _reloc_link_archive_fingerprint_token()
    fingerprint_rustflags = _append_rustflags_text(
        fingerprint_rustflags,
        f"--cfg molt_{'reloc' if reloc else 'shared'}_longdouble_link"
        f'="{_longdouble_link_token}"',
    )
    effective_stdlib_profile = stdlib_profile or DEFAULT_RUNTIME_STDLIB_PROFILE
    cargo_runtime_features = tuple(["wasm_freestanding"] if freestanding else [])
    builtin_features = _runtime_builtin_features_for_profile(
        effective_stdlib_profile,
        target_triple="wasm32-wasip1",
    )
    no_default_features, wasm_cargo_features, fingerprint_features = (
        _wasm_runtime_feature_plan(
            stdlib_profile=effective_stdlib_profile,
            runtime_features=cargo_runtime_features,
            builtin_features=builtin_features,
            resolved_modules=resolved_modules,
            required_link_features=required_link_features,
        )
    )
    fingerprint_path = _runtime_fingerprint_path(
        root, runtime_wasm, cargo_profile, "wasm32-wasip1"
    )
    if incremental_enabled:
        target_root = _runtime_wasm_incremental_target_root(
            root,
            _runtime_wasm_incremental_family_key(
                cargo_profile=cargo_profile,
                target_triple="wasm32-wasip1",
                features=tuple(fingerprint_features),
                simd_enabled=simd_enabled,
                freestanding=freestanding,
            ),
        )
    else:
        target_root = _cargo_target_root(root)
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    fingerprint = _runtime_fingerprint(
        root,
        cargo_profile=cargo_profile,
        target_triple="wasm32-wasip1",
        rustflags=fingerprint_rustflags,
        runtime_features=fingerprint_features,
        stored_fingerprint=stored_fingerprint,
    )
    return _RuntimeWasmBuildSpec(
        requested_cargo_profile=requested_cargo_profile,
        cargo_profile=cargo_profile,
        profile_dir=profile_dir,
        incremental_enabled=incremental_enabled,
        env=env,
        runtime_exports=runtime_exports,
        link_flags=link_flags,
        cargo_link_response_path=cargo_link_response_path,
        cargo_rustflags=cargo_rustflags,
        fingerprint_rustflags=fingerprint_rustflags,
        no_default_features=no_default_features,
        wasm_cargo_features=tuple(wasm_cargo_features),
        fingerprint_features=tuple(fingerprint_features),
        fingerprint_path=fingerprint_path,
        target_root=target_root,
        stored_fingerprint=stored_fingerprint,
        fingerprint=fingerprint,
    )


def _runtime_publication_bytes(
    data: bytes, *, reloc: bool, preserve_debug: bool
) -> bytes:
    if reloc:
        return data
    return strip_wasm_publication_sections(
        data, final_artifact=True, preserve_debug=preserve_debug
    )


def _ensure_runtime_wasm(
    runtime_wasm: Path,
    *,
    reloc: bool,
    json_output: bool,
    cargo_profile: str,
    cargo_timeout: float | None,
    project_root: Path | None = None,
    simd_enabled: bool = True,
    freestanding: bool = False,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: set[str] | frozenset[str] | None = None,
    required_link_features: frozenset[str] = frozenset(),
    required_exports: set[str] | frozenset[str] | None = None,
) -> bool:
    validate_exports = not reloc

    def _runtime_wasm_build_error_detail(
        build: subprocess.CompletedProcess[str],
    ) -> str | None:
        stderr = (build.stderr or "").strip()
        if stderr:
            return stderr
        stdout = (build.stdout or "").strip()
        if stdout:
            return stdout
        return None

    root = project_root or _compiler_root()
    # MOLT_SKIP_RUNTIME_REBUILD=1 skips the fingerprint check entirely.
    if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") == "1":
        if runtime_wasm.exists():
            runtime_valid = (
                _is_valid_runtime_wasm_artifact(runtime_wasm)
                if reloc
                else _is_valid_shared_runtime_wasm_artifact(runtime_wasm)
            )
            return (
                runtime_valid
                and _runtime_wasm_has_matching_integrity_pin(runtime_wasm)
                and (
                    not validate_exports
                    or _runtime_exports_satisfy_for_mode(
                        runtime_wasm,
                        required_exports,
                        reloc=reloc,
                    )
                )
            )
    (
        requested_cargo_profile,
        cargo_profile,
        profile_dir,
        incremental_enabled,
        env,
        runtime_exports,
        link_flags,
        cargo_link_response_path,
        cargo_rustflags,
        fingerprint_rustflags,
        no_default_features,
        wasm_cargo_features,
        fingerprint_features,
        fingerprint_path,
        target_root,
        stored_fingerprint,
        fingerprint,
    ) = _compute_runtime_wasm_build_spec(
        root,
        runtime_wasm,
        reloc=reloc,
        cargo_profile=cargo_profile,
        simd_enabled=simd_enabled,
        freestanding=freestanding,
        stdlib_profile=stdlib_profile,
        resolved_modules=resolved_modules,
        required_link_features=required_link_features,
        required_exports=required_exports,
    )
    if fingerprint is None:
        if not json_output:
            print("Failed to compute runtime wasm fingerprint.", file=sys.stderr)
        return False
    # FAIL LOUD for the witness/CPython-ABI tier: when this reloc runtime links
    # numpy/scipy long double, the long-double formatter + compiler-rt builtins
    # archives MUST be resolvable. Check BEFORE any reuse/hydrate/build path (the
    # shared-cache / compat-lattice / fingerprint-match reuse paths skip the
    # reloc link entirely) so a runtime that would relink the abort() stub and
    # trap at import can never be built OR served from cache. Micro / no-numpy
    # builds are unaffected (they degrade). The archive presence is folded into
    # the fingerprint, so a degraded (archives-absent) cached runtime is keyed
    # separately and only ever reused by other archives-absent builds â€” which
    # this gate then refuses for the numpy tier.
    long_double_required = reloc and _reloc_runtime_requires_long_double(
        resolved_modules=resolved_modules,
        required_exports=required_exports,
    )
    if long_double_required:
        _archives = _resolve_reloc_long_double_archives(long_double_required=True)
        if _archives.error is not None:
            if not json_output:
                print(_archives.error, file=sys.stderr)
            return False
    # V3 lattice index recorded alongside every published artifact: the
    # profile-independent ABI identity so a later iteration-profile request can
    # find this artifact as compatible-or-better (see runtime_wasm_cache).
    _compat_key = {
        "inputs_digest": fingerprint.get("inputs_digest"),
        "compat_digest": _runtime_wasm_compat_digest(
            target_triple="wasm32-wasip1",
            rustflags=fingerprint_rustflags,
            features=fingerprint_features,
        ),
        "cargo_profile": cargo_profile,
    }

    def _publish_runtime_integrity_pin() -> None:
        preserve_debug = any(
            marker in profile_name.lower()
            for profile_name in (requested_cargo_profile, cargo_profile)
            for marker in ("dev", "debug")
        )
        published = runtime_wasm.read_bytes()
        stripped = _runtime_publication_bytes(
            published,
            reloc=reloc,
            preserve_debug=preserve_debug,
        )
        if stripped != published:
            _atomic_write_bytes(runtime_wasm, stripped)
        # One integrity-pin slot per resolved build identity: the fingerprint
        # meta digest keys the sidecar so different-profile builds never
        # contend for a single pinned hash.
        _write_runtime_wasm_integrity_sidecar(
            runtime_wasm, integrity_key=_runtime_wasm_integrity_key(fingerprint)
        )

    lock_suffix = "reloc" if reloc else "shared"
    lock_name = f"runtime.{cargo_profile}.wasm32-wasip1.{lock_suffix}"
    with _build_lock(root, lock_name):
        if stored_fingerprint is None:
            stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
        target_label = "wasm32-wasip1"
        target_build_state_root = _build_state_root(root)
        target_runtime_wasm_current = _current_runtime_target_artifact(
            _wasm_runtime_wasm_candidates(target_root, profile_dir),
            build_state_root=target_build_state_root,
            cargo_profile=cargo_profile,
            target_label=target_label,
            fingerprint=fingerprint,
        )
        if (
            not reloc
            and target_runtime_wasm_current is not None
            and (target_runtime_wasm := target_runtime_wasm_current[0])
            and _inspect_wasm_binary(target_runtime_wasm) == "valid"
            and _is_valid_shared_runtime_wasm_artifact(target_runtime_wasm)
        ):
            assert fingerprint is not None
            _record_runtime_wasm_build_phase(
                "cargo_compile",
                0.0,
                kind="shared",
                mode="target_reuse",
                detail="cdylib reused from cargo target dir",
            )
            target_runtime_wasm_fingerprint_path = target_runtime_wasm_current[1]
            runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
            _atomic_copy_file(target_runtime_wasm, runtime_wasm)
            if _inspect_wasm_binary(runtime_wasm) != "valid":
                if not json_output:
                    print(
                        f"Copied runtime wasm artifact is invalid: {runtime_wasm}",
                        file=sys.stderr,
                    )
                return False
            # Validate only AFTER materialization.  The target-dir cdylib holds
            # rustc's raw #[no_mangle] names while the shipped shared artifact
            # renames the CPython-ABI subset (for example `PyBool_Check` to
            # `molt_PyBool_Check`).  Rejecting the raw target before this step
            # made every combined compile fall through to a redundant second
            # cargo compile even though it had already produced the canonical
            # crate-type pair.
            if not _materialize_split_runtime_public_exports(
                runtime_wasm,
                required_exports,
                json_output=json_output,
            ):
                return False
            reused_missing_exports = _runtime_missing_exports_for_mode(
                runtime_wasm,
                required_exports,
                reloc=reloc,
            )
            if reused_missing_exports:
                if not json_output:
                    print(
                        "Reused runtime wasm artifact missing required exports: "
                        + ", ".join(sorted(reused_missing_exports)),
                        file=sys.stderr,
                    )
                return False
            try:
                _publish_runtime_integrity_pin()
                target_runtime_wasm_fingerprint_path.parent.mkdir(
                    parents=True,
                    exist_ok=True,
                )
                _write_runtime_fingerprint(
                    target_runtime_wasm_fingerprint_path,
                    fingerprint,
                    artifact=target_runtime_wasm,
                )
                fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(
                    fingerprint_path,
                    fingerprint,
                    artifact=runtime_wasm,
                )
            except OSError:
                if not json_output:
                    print(
                        "Failed to publish prebuilt runtime wasm metadata.",
                        file=sys.stderr,
                    )
                return False
            return True

        def _finalize_reused_runtime_wasm() -> bool:
            # Shared reuse/hydration lands a validated artifact at
            # ``runtime_wasm``; record the integrity pin + fingerprint sidecar so
            # the normal fast-path recognizes it on the next call without a
            # rebuild.
            assert fingerprint is not None
            try:
                _publish_runtime_integrity_pin()
                fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(
                    fingerprint_path,
                    fingerprint,
                    artifact=runtime_wasm,
                )
            except OSError:
                if not json_output:
                    print(
                        "Failed to publish reused runtime wasm metadata.",
                        file=sys.stderr,
                    )
                return False
            return True

        # Cross-session shared cache: the session-local target dir missed (a
        # fresh session/worktree always starts cold), but a byte-identical
        # runtime wasm for this exact content-addressed build identity may
        # already sit in the session-independent shared cache. Reuse it instead
        # of recompiling the entire runtime crate. The app.wasm build stays
        # session-scoped; only this fixed runtime artifact is shared.
        _shared_cache_validator = (
            _is_valid_runtime_wasm_artifact
            if reloc
            else _is_valid_shared_runtime_wasm_artifact
        )
        if _hydrate_runtime_wasm_from_shared_cache(
            dest=runtime_wasm,
            fingerprint=fingerprint,
            reloc=reloc,
            is_valid=_shared_cache_validator,
        ) and (
            not validate_exports
            or _runtime_exports_satisfy_for_mode(
                runtime_wasm,
                required_exports,
                reloc=reloc,
            )
        ):
            if _finalize_reused_runtime_wasm():
                _record_runtime_wasm_build_phase(
                    "cargo_compile",
                    0.0,
                    kind="reloc" if reloc else "shared",
                    mode="shared_cache",
                    detail="hydrated from session-independent shared cache",
                )
                return True

        # V3 config-lattice reuse (opt-in): the exact-identity cache missed, but
        # an iteration request may be served by a SAME-SOURCE artifact built at a
        # compatible-or-better opt level (the consumer only observes the export/
        # import ABI, not the profile). Acceptance lanes keep this OFF and pin
        # exact identity. The candidate is re-validated (structure + exports).
        if (
            _build_reuse_compatible_enabled()
            and _hydrate_runtime_wasm_from_compatible_cache(
                dest=runtime_wasm,
                reloc=reloc,
                inputs_digest=_compat_key["inputs_digest"],
                compat_digest=str(_compat_key["compat_digest"]),
                request_profile=cargo_profile,
                is_valid=_shared_cache_validator,
                exports_ok=lambda path: (
                    not validate_exports
                    or _runtime_exports_satisfy_for_mode(
                        path, required_exports, reloc=reloc
                    )
                ),
            )
        ):
            if _finalize_reused_runtime_wasm():
                _record_runtime_wasm_build_phase(
                    "cargo_compile",
                    0.0,
                    kind="reloc" if reloc else "shared",
                    mode="compat_lattice",
                    detail="reused compatible-or-better-opt artifact (V3 lattice)",
                )
                return True

        target_runtime_staticlib_current = _current_runtime_target_artifact(
            _wasm_runtime_staticlib_candidates(target_root, profile_dir),
            build_state_root=target_build_state_root,
            cargo_profile=cargo_profile,
            target_label=target_label,
            fingerprint=fingerprint,
        )
        if reloc and target_runtime_staticlib_current is not None:
            assert fingerprint is not None
            target_runtime_staticlib, target_runtime_staticlib_fingerprint_path = (
                target_runtime_staticlib_current
            )
            _record_runtime_wasm_build_phase(
                "cargo_compile",
                0.0,
                kind="reloc",
                mode="target_reuse",
                detail="staticlib reused from cargo target dir",
            )
            _reloc_link_started = time.perf_counter()
            if not _link_runtime_staticlib_to_reloc_wasm(
                staticlib_path=target_runtime_staticlib,
                output_path=runtime_wasm,
                json_output=json_output,
                link_timeout=cargo_timeout,
                export_link_args=runtime_exports,
                long_double_required=long_double_required,
            ):
                return False
            _record_runtime_wasm_build_phase(
                "reloc_link",
                time.perf_counter() - _reloc_link_started,
                kind="reloc",
                mode="link",
            )
            try:
                _publish_runtime_integrity_pin()
                target_runtime_staticlib_fingerprint_path.parent.mkdir(
                    parents=True,
                    exist_ok=True,
                )
                _write_runtime_fingerprint(
                    target_runtime_staticlib_fingerprint_path,
                    fingerprint,
                    artifact=target_runtime_staticlib,
                )
                fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(
                    fingerprint_path,
                    fingerprint,
                    artifact=runtime_wasm,
                )
            except OSError:
                if not json_output:
                    print(
                        "Failed to publish prebuilt runtime wasm metadata.",
                        file=sys.stderr,
                    )
                return False
            return True

        needs_rebuild = not _runtime_artifact_fingerprint_matches(
            runtime_wasm,
            fingerprint,
            fingerprint_path,
            require_artifact_digest=True,
        )
        if (
            not needs_rebuild
            and (
                _is_valid_runtime_wasm_artifact(runtime_wasm)
                if reloc
                else _is_valid_shared_runtime_wasm_artifact(runtime_wasm)
            )
            and (
                not validate_exports
                or _runtime_exports_satisfy_for_mode(
                    runtime_wasm,
                    required_exports,
                    reloc=reloc,
                )
            )
        ):
            assert fingerprint is not None
            try:
                _publish_runtime_integrity_pin()
                fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(
                    fingerprint_path,
                    fingerprint,
                    artifact=runtime_wasm,
                )
            except OSError:
                if not json_output:
                    print(
                        "Failed to update runtime wasm integrity sidecar.",
                        file=sys.stderr,
                    )
                return False
            return True
        if (
            not needs_rebuild
            and validate_exports
            and not _runtime_exports_satisfy_for_mode(
                runtime_wasm,
                required_exports,
                reloc=reloc,
            )
            and not json_output
        ):
            print(
                "Runtime wasm artifact missing required exports; forcing rebuild.",
                file=sys.stderr,
            )
        elif not needs_rebuild and not json_output:
            print(
                "Runtime wasm artifact invalid/corrupt; forcing rebuild.",
                file=sys.stderr,
            )
        if wasm_toolchain.rust_target_libdir("wasm32-wasip1") is None:
            if not json_output:
                print(
                    wasm_toolchain.rust_target_missing_message(
                        "wasm32-wasip1",
                        root=root,
                        context="Runtime wasm build",
                    ),
                    file=sys.stderr,
                )
            return False
        if not json_output:
            print("Runtime sources changed; rebuilding runtime...", file=sys.stderr)
        if cargo_rustflags:
            env["RUSTFLAGS"] = cargo_rustflags
        if os.environ.get("MOLT_WASM_FORCE_CC") == "1":
            _configure_wasm_cc_env(env)
        _configure_wasi_sysroot_env(env)
        _configure_wasm_long_double_env(env)
        # Deterministic proof builds default Cargo incremental off at the env
        # boundary; an explicit operator-provided CARGO_INCREMENTAL remains
        # authoritative for local incremental-debug sessions.
        # Enable sccache for WASM builds by default (same as native builds).
        # Set MOLT_WASM_DISABLE_SCCACHE=1 to opt out.
        if os.environ.get("MOLT_WASM_DISABLE_SCCACHE") != "1":
            _maybe_enable_sccache(env)
        else:
            env.pop("RUSTC_WRAPPER", None)
        if reloc:
            cmd = [
                "cargo",
                "rustc",
                "--package",
                "molt-runtime",
                "--profile",
                cargo_profile,
                "--target",
                "wasm32-wasip1",
                "--lib",
            ]
        else:
            cmd = [
                "cargo",
                "rustc",
                "--package",
                "molt-runtime",
                "--profile",
                cargo_profile,
                "--target",
                "wasm32-wasip1",
                "--lib",
            ]
        if no_default_features:
            cmd.append("--no-default-features")
        if wasm_cargo_features:
            cmd.extend(["--features", ",".join(wasm_cargo_features)])
        if reloc:
            cmd.extend(["--", "--crate-type=staticlib"])
        else:
            cmd.extend(["--", "--crate-type=cdylib"])
            if cargo_link_response_path is not None:
                cmd.extend(["-C", f"link-arg=@{cargo_link_response_path}"])
        _cargo_compile_started = time.perf_counter()
        try:
            build, src = _run_runtime_wasm_cargo_build(
                cmd=cmd,
                root=root,
                env=env,
                cargo_timeout=cargo_timeout,
                profile_dir=profile_dir,
                target_root_override=target_root,
                json_output=json_output,
                artifact_kind="staticlib" if reloc else "cdylib",
            )
        except subprocess.TimeoutExpired:
            if not json_output:
                timeout_note = (
                    f"Runtime wasm build timed out after {cargo_timeout:.1f}s."
                    if cargo_timeout is not None
                    else "Runtime wasm build timed out."
                )
                print(timeout_note, file=sys.stderr)
            return False
        if build.returncode != 0:
            detail = _runtime_wasm_build_error_detail(build)
            msg = "Runtime wasm build failed"
            if detail:
                msg = f"{msg}: {detail}"
            print(msg, file=sys.stderr)
            return False
        _record_runtime_wasm_build_phase(
            "cargo_compile",
            time.perf_counter() - _cargo_compile_started,
            kind="reloc" if reloc else "shared",
            mode="build",
            detail=(
                "target_dir=stable-incremental (cross-session dep cache)"
                if incremental_enabled
                else "target_dir=session"
            ),
        )
        if reloc:
            if not src.exists():
                if not json_output:
                    print(
                        "Runtime wasm build succeeded but staticlib artifact is missing.",
                        file=sys.stderr,
                    )
                return False
            _reloc_link_started = time.perf_counter()
            if not _link_runtime_staticlib_to_reloc_wasm(
                staticlib_path=src,
                output_path=runtime_wasm,
                json_output=json_output,
                link_timeout=cargo_timeout,
                export_link_args=runtime_exports,
                long_double_required=long_double_required,
            ):
                return False
            _record_runtime_wasm_build_phase(
                "reloc_link",
                time.perf_counter() - _reloc_link_started,
                kind="reloc",
                mode="link",
            )
            try:
                _publish_runtime_integrity_pin()
            except OSError:
                if not json_output:
                    print(
                        "Failed to update runtime wasm integrity sidecar.",
                        file=sys.stderr,
                    )
                return False
            if fingerprint is not None:
                fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(
                    fingerprint_path,
                    fingerprint,
                    artifact=runtime_wasm,
                )
                reported_staticlib_fingerprint_path = _runtime_target_fingerprint_path(
                    target_build_state_root,
                    src,
                    cargo_profile=cargo_profile,
                    target_label=target_label,
                )
                reported_staticlib_fingerprint_path.parent.mkdir(
                    parents=True,
                    exist_ok=True,
                )
                _write_runtime_fingerprint(
                    reported_staticlib_fingerprint_path,
                    fingerprint,
                    artifact=src,
                )
                # Publish the freshly built reloc runtime wasm to the shared,
                # session-independent cache so the next fresh session/worktree
                # reuses it instead of recompiling the runtime crate.
                # Incremental iteration builds are NOT published: they share the
                # fingerprint of a non-incremental release build, so publishing
                # would let an incrementally-compiled artifact hydrate an
                # acceptance / final-green run (M05). The per-family incremental
                # target dir already provides cross-iteration reuse.
                if not incremental_enabled:
                    _warn_runtime_wasm_cache_publish_failure(
                        _publish_runtime_wasm_to_shared_cache(
                            src=runtime_wasm,
                            fingerprint=fingerprint,
                            reloc=reloc,
                            compat=_compat_key,
                        ),
                        json_output=json_output,
                    )
            return True
        src_state = _inspect_wasm_binary(src)
        if src_state == "missing":
            if not json_output:
                print(
                    "Runtime wasm build succeeded but artifact is missing.",
                    file=sys.stderr,
                )
            return False
        if src_state != "valid":
            if not json_output:
                print(
                    f"Runtime wasm build produced invalid artifact: {src}; retrying with isolated target dir.",
                    file=sys.stderr,
                )
            recovery_target_root = _wasm_runtime_recovery_target_root(
                _cargo_target_root(root)
            )
            try:
                build, recovery_src = _run_runtime_wasm_cargo_build(
                    cmd=cmd,
                    root=root,
                    env=env,
                    cargo_timeout=cargo_timeout,
                    profile_dir=profile_dir,
                    target_root_override=recovery_target_root,
                    json_output=json_output,
                )
            except subprocess.TimeoutExpired:
                if not json_output:
                    timeout_note = (
                        f"Runtime wasm recovery build timed out after {cargo_timeout:.1f}s."
                        if cargo_timeout is not None
                        else "Runtime wasm recovery build timed out."
                    )
                    print(timeout_note, file=sys.stderr)
                return False
            if build.returncode != 0:
                if not json_output:
                    detail = _runtime_wasm_build_error_detail(build)
                    msg = "Runtime wasm recovery build failed"
                    if detail:
                        msg = f"{msg}: {detail}"
                    print(msg, file=sys.stderr)
                return False
            recovery_state = _inspect_wasm_binary(recovery_src)
            if recovery_state == "missing":
                if not json_output:
                    print(
                        "Runtime wasm recovery build succeeded but artifact is missing.",
                        file=sys.stderr,
                    )
                return False
            if recovery_state != "valid":
                # The wasm fallback MUST preserve wasm-release's size + panic
                # contract (opt size, panic=abort, strip). The previous default
                # `release-fast` (opt-3, panic=unwind) re-introduced wasm unwind
                # tables and inflated the runtime past the 3 MB Cloudflare
                # ceiling - a workaround, not a recovery. `wasm-release-fallback`
                # (Cargo.toml) keeps opt-"s"/abort/strip and only relaxes the
                # codegen knobs (thin LTO + 16 codegen-units) to escape the
                # fat-LTO single-CGU corruption class a fallback recovers from.
                fallback_profile = os.environ.get(
                    "MOLT_WASM_RUNTIME_FALLBACK_PROFILE", "wasm-release-fallback"
                ).strip()
                can_try_fallback_profile = (
                    requested_cargo_profile == "release"
                    and fallback_profile
                    and fallback_profile != cargo_profile
                    and _CARGO_PROFILE_NAME_RE.match(fallback_profile) is not None
                )
                if not can_try_fallback_profile:
                    if not json_output:
                        print(
                            f"Runtime wasm recovery build produced invalid artifact: {recovery_src}",
                            file=sys.stderr,
                        )
                    return False
                if not json_output:
                    print(
                        "Runtime wasm release profile produced invalid artifacts; "
                        f"retrying with fallback profile {fallback_profile}.",
                        file=sys.stderr,
                    )
                fallback_profile_dir = _cargo_profile_dir(fallback_profile)
                fallback_cmd = cmd.copy()
                fallback_cmd[5] = fallback_profile
                fallback_target_root = recovery_target_root.parent / (
                    f"{recovery_target_root.name}-{fallback_profile}"
                )
                try:
                    build, fallback_src = _run_runtime_wasm_cargo_build(
                        cmd=fallback_cmd,
                        root=root,
                        env=env,
                        cargo_timeout=cargo_timeout,
                        profile_dir=fallback_profile_dir,
                        target_root_override=fallback_target_root,
                        json_output=json_output,
                    )
                except subprocess.TimeoutExpired:
                    if not json_output:
                        timeout_note = (
                            f"Runtime wasm fallback build timed out after {cargo_timeout:.1f}s."
                            if cargo_timeout is not None
                            else "Runtime wasm fallback build timed out."
                        )
                        print(timeout_note, file=sys.stderr)
                    return False
                if build.returncode != 0:
                    if not json_output:
                        detail = _runtime_wasm_build_error_detail(build)
                        msg = "Runtime wasm fallback build failed"
                        if detail:
                            msg = f"{msg}: {detail}"
                        print(msg, file=sys.stderr)
                    return False
                fallback_state = _inspect_wasm_binary(fallback_src)
                if fallback_state == "missing":
                    if not json_output:
                        print(
                            "Runtime wasm fallback build succeeded but artifact is missing.",
                            file=sys.stderr,
                        )
                    return False
                if fallback_state != "valid":
                    if not json_output:
                        print(
                            f"Runtime wasm fallback build produced invalid artifact: {fallback_src}",
                            file=sys.stderr,
                        )
                    return False
                src = fallback_src
            else:
                src = recovery_src
        if reloc:
            missing_exports = _runtime_missing_exports_for_mode(
                src,
                required_exports,
                reloc=reloc,
            )
            if missing_exports:
                if not json_output:
                    print(
                        "Runtime wasm build produced artifact missing required exports: "
                        + ", ".join(sorted(missing_exports)),
                        file=sys.stderr,
                    )
                return False
        if not _is_valid_shared_runtime_wasm_artifact(src):
            if not json_output:
                print(
                    "Runtime wasm build produced artifact missing shared "
                    "memory/table import ABI.",
                    file=sys.stderr,
                )
            return False
        runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
        _atomic_copy_file(src, runtime_wasm)
        if _inspect_wasm_binary(runtime_wasm) != "valid":
            if not json_output:
                print(
                    f"Copied runtime wasm artifact is invalid: {runtime_wasm}",
                    file=sys.stderr,
                )
            return False
        if not reloc and not _materialize_split_runtime_public_exports(
            runtime_wasm,
            required_exports,
            json_output=json_output,
        ):
            return False
        try:
            missing_exports = _runtime_missing_exports_for_mode(
                runtime_wasm,
                required_exports,
                reloc=reloc,
            )
            if missing_exports:
                if not json_output:
                    print(
                        "Runtime wasm build produced artifact missing required exports: "
                        + ", ".join(sorted(missing_exports)),
                        file=sys.stderr,
                    )
                return False
            _publish_runtime_integrity_pin()
        except OSError:
            if not json_output:
                print(
                    "Failed to update runtime wasm integrity sidecar.",
                    file=sys.stderr,
                )
            return False
        if fingerprint is not None:
            try:
                reported_wasm_fingerprint_path = _runtime_target_fingerprint_path(
                    target_build_state_root,
                    src,
                    cargo_profile=cargo_profile,
                    target_label=target_label,
                )
                reported_wasm_fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(
                    reported_wasm_fingerprint_path,
                    fingerprint,
                    artifact=src,
                )
                fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(
                    fingerprint_path,
                    fingerprint,
                    artifact=runtime_wasm,
                )
                # Publish the freshly built shared runtime wasm to the
                # session-independent shared cache so the next fresh
                # session/worktree reuses it instead of recompiling the runtime
                # crate from a cold per-session target dir.
                # Incremental iteration builds are NOT published (see the reloc
                # branch): they share the release fingerprint, so publishing
                # would leak an incrementally-compiled artifact into an
                # acceptance / final-green hydrate (M05).
                if not incremental_enabled:
                    _warn_runtime_wasm_cache_publish_failure(
                        _publish_runtime_wasm_to_shared_cache(
                            src=runtime_wasm,
                            fingerprint=fingerprint,
                            reloc=reloc,
                            compat=_compat_key,
                        ),
                        json_output=json_output,
                    )
            except OSError:
                if not json_output:
                    print(
                        "Warning: failed to write runtime fingerprint metadata.",
                        file=sys.stderr,
                    )
    return True


def _single_compile_split_runtime_enabled() -> bool:
    """Whether ``--kind both`` uses ONE combined cargo compile (V1 dedup).

    Default ON.  Kill switch ``MOLT_RUNTIME_WASM_SINGLE_COMPILE=0`` reverts to the
    proven sequential dual-compile (two ``cargo rustc --crate-type=...`` passes),
    the fallback also taken automatically whenever the combined compile fails.
    """
    return os.environ.get(
        "MOLT_RUNTIME_WASM_SINGLE_COMPILE", "1"
    ).strip().lower() not in {"0", "false", "no", "off"}


def _prepopulate_combined_runtime_wasm_target(
    *,
    shared_spec: _RuntimeWasmBuildSpec,
    reloc_spec: _RuntimeWasmBuildSpec,
    json_output: bool,
    cargo_timeout: float | None,
    project_root: Path,
    simd_enabled: bool,
    freestanding: bool,
) -> bool:
    """Run ONE ``cargo rustc --lib`` emitting BOTH staticlib and cdylib (V1).

    ``molt-runtime`` declares ``crate-type = ["staticlib","rlib","cdylib"]`` so a
    single rustc codegen emits every crate-type.  The shared cdylib link args
    (``--import-memory``/``--import-table``/``--growable-table`` plus the
    ``--export-if-defined`` allowlist) are passed as ``-- -C link-arg=@resp``
    extra rustc args; rustc applies them only to the cdylib link (the staticlib
    is an archive), preserving rustc own authoritative ``#[no_mangle]`` cdylib
    export enumeration -- the ~3260 exports the reverted hand-link dropped.

    Both target-dir fingerprints are then recorded so the per-artifact reuse
    fast path in ``_ensure_runtime_wasm`` finalises each artifact (export
    materialization, integrity pin, sidecars) WITHOUT a second compile.  Returns
    ``True`` when the target dir now satisfies both crate-types, else ``False``
    (caller falls back to the sequential dual-compile -- correct, just no dedup).
    """
    root = project_root
    if shared_spec.fingerprint is None or reloc_spec.fingerprint is None:
        return False
    # target_root/profile are codegen-identity properties: identical for the
    # reloc and shared specs (features + codegen rustflags do not depend on the
    # link-only crate-type), so either spec names the shared compile home.
    cargo_profile = shared_spec.cargo_profile
    profile_dir = shared_spec.profile_dir
    target_root = shared_spec.target_root
    build_state_root = _build_state_root(root)
    target_label = "wasm32-wasip1"

    cdylib_current = _current_runtime_target_artifact(
        _wasm_runtime_wasm_candidates(target_root, profile_dir),
        build_state_root=build_state_root,
        cargo_profile=cargo_profile,
        target_label=target_label,
        fingerprint=shared_spec.fingerprint,
    )
    staticlib_current = _current_runtime_target_artifact(
        _wasm_runtime_staticlib_candidates(target_root, profile_dir),
        build_state_root=build_state_root,
        cargo_profile=cargo_profile,
        target_label=target_label,
        fingerprint=reloc_spec.fingerprint,
    )
    if cdylib_current is not None and staticlib_current is not None:
        # Both crate-types already fresh in the target dir; nothing to compile.
        return True

    if wasm_toolchain.rust_target_libdir("wasm32-wasip1") is None:
        if not json_output:
            print(
                wasm_toolchain.rust_target_missing_message(
                    "wasm32-wasip1",
                    root=root,
                    context="Runtime wasm combined build",
                ),
                file=sys.stderr,
            )
        return False

    env = dict(shared_spec.env)
    # RUSTFLAGS carries ONLY the codegen flags (target-feature); the cdylib link
    # args move to `-- -C link-arg=@resp` so the compile fingerprint is not
    # crate-type/link-arg specific and one compile serves both artifacts.
    codegen_rustflags = _wasm_runtime_codegen_rustflags(
        env.get("RUSTFLAGS", "").strip(),
        simd_enabled=simd_enabled,
        freestanding=freestanding,
    )
    if codegen_rustflags:
        env["RUSTFLAGS"] = codegen_rustflags
    else:
        env.pop("RUSTFLAGS", None)
    if os.environ.get("MOLT_WASM_FORCE_CC") == "1":
        _configure_wasm_cc_env(env)
    _configure_wasi_sysroot_env(env)
    _configure_wasm_long_double_env(env)
    if os.environ.get("MOLT_WASM_DISABLE_SCCACHE") != "1":
        _maybe_enable_sccache(env)
    else:
        env.pop("RUSTC_WRAPPER", None)

    cmd = [
        "cargo",
        "rustc",
        "--package",
        "molt-runtime",
        "--profile",
        cargo_profile,
        "--target",
        "wasm32-wasip1",
        "--lib",
    ]
    if shared_spec.no_default_features:
        cmd.append("--no-default-features")
    if shared_spec.wasm_cargo_features:
        cmd.extend(["--features", ",".join(shared_spec.wasm_cargo_features)])
    # NO `--crate-type` override: build every declared crate-type in one compile.
    cmd.append("--")
    shared_link_args = _wasm_link_args_from_rustflags(shared_spec.link_flags)
    if shared_link_args:
        response_path = _write_wasm_link_args_response_file(
            _build_state_root(root) / "wasm_link_args",
            label=f"runtime.{cargo_profile}.combined",
            link_args=shared_link_args,
        )
        cmd.extend(["-C", f"link-arg=@{response_path}"])

    if not json_output:
        print(
            "Building runtime wasm (single combined compile: staticlib+cdylib)...",
            file=sys.stderr,
        )
    started = time.perf_counter()
    try:
        build, _reported = _run_runtime_wasm_cargo_build(
            cmd=cmd,
            root=root,
            env=env,
            cargo_timeout=cargo_timeout,
            profile_dir=profile_dir,
            target_root_override=target_root,
            json_output=json_output,
            artifact_kind="cdylib",
        )
    except subprocess.TimeoutExpired:
        if not json_output:
            print("Runtime wasm combined build timed out.", file=sys.stderr)
        return False
    if build.returncode != 0:
        if not json_output:
            detail = (build.stderr or build.stdout or "").strip()
            msg = "Runtime wasm combined build failed"
            if detail:
                msg = f"{msg}: {detail}"
            print(msg, file=sys.stderr)
        return False
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

    cdylib_candidates = _wasm_runtime_wasm_candidates(target_root, profile_dir)
    staticlib_candidates = _wasm_runtime_staticlib_candidates(target_root, profile_dir)
    if not cdylib_candidates or not staticlib_candidates:
        if not json_output:
            print(
                "Runtime wasm combined build succeeded but a crate-type artifact "
                "is missing (expected both cdylib and staticlib).",
                file=sys.stderr,
            )
        return False
    cdylib = cdylib_candidates[0]
    staticlib = staticlib_candidates[0]
    if _inspect_wasm_binary(
        cdylib
    ) != "valid" or not _is_valid_shared_runtime_wasm_artifact(cdylib):
        if not json_output:
            print(
                "Runtime wasm combined build produced an invalid cdylib artifact.",
                file=sys.stderr,
            )
        return False

    try:
        cdylib_fp_path = _runtime_target_fingerprint_path(
            build_state_root,
            cdylib,
            cargo_profile=cargo_profile,
            target_label=target_label,
        )
        cdylib_fp_path.parent.mkdir(parents=True, exist_ok=True)
        _write_runtime_fingerprint(
            cdylib_fp_path, shared_spec.fingerprint, artifact=cdylib
        )
        staticlib_fp_path = _runtime_target_fingerprint_path(
            build_state_root,
            staticlib,
            cargo_profile=cargo_profile,
            target_label=target_label,
        )
        staticlib_fp_path.parent.mkdir(parents=True, exist_ok=True)
        _write_runtime_fingerprint(
            staticlib_fp_path, reloc_spec.fingerprint, artifact=staticlib
        )
    except OSError:
        if not json_output:
            print(
                "Runtime wasm combined build: failed to record target fingerprints.",
                file=sys.stderr,
            )
        return False
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
) -> bool:
    """Ensure BOTH the shared (cdylib) and reloc (staticlib) runtime wasm.

    V1 dual-compile burn-down: when single-compile is enabled (default), one
    combined ``cargo rustc --lib`` populates the target dir with both crate-types
    (``_prepopulate_combined_runtime_wasm_target``); the two per-artifact
    ``_ensure_runtime_wasm_artifact`` calls then finalise each via the UNCHANGED
    reuse path (no second compile).  On any combined-build failure the per-artifact
    calls transparently recompile, so behaviour degrades to the proven sequential
    dual-compile -- never incorrect, only un-deduped.
    """
    runtime_wasm = runtime_state.runtime_wasm
    runtime_reloc_wasm = runtime_state.runtime_reloc_wasm

    def _ensure(reloc: bool) -> bool:
        return _ensure_runtime_wasm_artifact(
            runtime_state,
            reloc=reloc,
            json_output=json_output,
            cargo_profile=cargo_profile,
            cargo_timeout=cargo_timeout,
            project_root=project_root,
            simd_enabled=simd_enabled,
            freestanding=freestanding,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
            required_link_features=required_link_features,
            required_exports=required_exports,
        )

    if (
        _single_compile_split_runtime_enabled()
        and runtime_wasm is not None
        and runtime_reloc_wasm is not None
    ):
        shared_spec = _compute_runtime_wasm_build_spec(
            project_root,
            runtime_wasm,
            reloc=False,
            cargo_profile=cargo_profile,
            simd_enabled=simd_enabled,
            freestanding=freestanding,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
            required_link_features=required_link_features,
            required_exports=required_exports,
        )
        reloc_spec = _compute_runtime_wasm_build_spec(
            project_root,
            runtime_reloc_wasm,
            reloc=True,
            cargo_profile=cargo_profile,
            simd_enabled=simd_enabled,
            freestanding=freestanding,
            stdlib_profile=stdlib_profile,
            resolved_modules=resolved_modules,
            required_link_features=required_link_features,
            required_exports=required_exports,
        )
        # Best-effort: populate the target dir with one compile. A False result
        # (build failed / toolchain missing) just means the per-artifact calls
        # below recompile as before -- correctness is unaffected.
        _prepopulate_combined_runtime_wasm_target(
            shared_spec=shared_spec,
            reloc_spec=reloc_spec,
            json_output=json_output,
            cargo_timeout=cargo_timeout,
            project_root=project_root,
            simd_enabled=simd_enabled,
            freestanding=freestanding,
        )

    ok_shared = _ensure(reloc=False)
    ok_reloc = _ensure(reloc=True)
    return ok_shared and ok_reloc
