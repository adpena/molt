from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from typing import Collection, Mapping

from molt.cli.artifact_state import (
    _artifact_state_path_for_build_state_root,
    _canonical_build_state_root,
    _canonical_target_root,
    _maybe_hydrate_artifact_from_canonical_target,
    _runtime_fingerprint_path,
)
from molt.cli.atomic_io import _atomic_copy_file
from molt.cli.build_locks import _build_lock
from molt.cli.capability_spec import _dedupe_preserve_order
from molt.cli.cargo_execution import (
    _build_slot,
    _cargo_build_env,
    _maybe_enable_sccache,
    _run_cargo_with_sccache_retry,
)
from molt.cli.runtime_features import (
    _runtime_builtin_features_for_profile,
    _runtime_cargo_features,
)
from molt.cli.runtime_fingerprints import (
    _read_runtime_fingerprint,
    _runtime_artifact_fingerprint_matches,
    _runtime_fingerprint,
    _write_runtime_fingerprint,
)
from molt.cli.runtime_paths import (
    _cargo_profile_dir,
    _cargo_target_root,
    _runtime_cargo_scratch_lib_path,
    _runtime_lib_path,
    _runtime_wasm_artifact_path,
)
from molt.cli.models import _RuntimeArtifactState


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


def _initialize_runtime_artifact_state(
    *,
    is_rust_transpile: bool,
    is_wasm: bool,
    emit_mode: str,
    molt_root: Path,
    runtime_cargo_profile: str,
    target_triple: str | None,
    stdlib_profile: str | None = "micro",
) -> _RuntimeArtifactState:
    state = _RuntimeArtifactState()
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
    stdlib_profile: str | None = "micro",
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
    stdlib_profile: str | None = "micro",
    resolved_modules: Collection[str] | None = None,
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
    stdlib_profile: str | None = "micro",
    resolved_modules: set[str] | frozenset[str] | None = None,
) -> bool:
    runtime_lib = runtime_state.runtime_lib
    if runtime_lib is None:
        return True
    if runtime_state.runtime_lib_ready_future is not None:
        if diagnostics_enabled and "runtime_setup" not in phase_starts:
            phase_starts["runtime_setup"] = time.perf_counter()
        try:
            return bool(runtime_state.runtime_lib_ready_future.result())
        finally:
            runtime_state.runtime_lib_ready_future = None
    if diagnostics_enabled and "runtime_setup" not in phase_starts:
        phase_starts["runtime_setup"] = time.perf_counter()
    return _ensure_runtime_lib_ready(
        runtime_state,
        target_triple=target_triple,
        json_output=json_output,
        runtime_cargo_profile=runtime_cargo_profile,
        molt_root=molt_root,
        cargo_timeout=cargo_timeout,
        stdlib_profile=stdlib_profile,
        resolved_modules=resolved_modules,
    )


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


def _ensure_runtime_lib(
    runtime_lib: Path,
    target_triple: str | None,
    json_output: bool,
    cargo_profile: str,
    project_root: Path,
    cargo_timeout: float | None,
    stdlib_profile: str | None = "micro",
    resolved_modules: Collection[str] | None = None,
) -> bool:
    rustflags = os.environ.get("RUSTFLAGS", "")
    runtime_features = _runtime_cargo_features(target_triple)
    builtin_features = _runtime_builtin_features_for_profile(
        stdlib_profile,
        target_triple=target_triple,
    )
    # Cargo writes the platform staticlib name as scratch output. Molt then
    # materializes a profile-qualified link alias, so the requested feature
    # profile must remain an explicit fingerprint input.
    fingerprint_features: tuple[str, ...] = tuple(
        _dedupe_preserve_order(
            list(runtime_features) + ["stdlib_full", "default-features"]
        )
    )
    if stdlib_profile == "micro":
        fingerprint_features = tuple(
            _dedupe_preserve_order(
                list(runtime_features)
                + sorted(builtin_features)
                + ["stdlib_micro", "no-default-features"]
            )
        )
    fingerprint_path = _runtime_fingerprint_path(
        project_root, runtime_lib, cargo_profile, target_triple
    )
    # MOLT_SKIP_RUNTIME_REBUILD=1 skips the fingerprint check entirely.
    # Use when you have already run `cargo build` manually and want to avoid
    # the ~90s overhead of the CLI re-running cargo.
    if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") == "1":
        if runtime_lib.exists():
            return True
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    fingerprint = _runtime_fingerprint(
        project_root,
        cargo_profile=cargo_profile,
        target_triple=target_triple,
        rustflags=rustflags,
        runtime_features=fingerprint_features,
        stored_fingerprint=stored_fingerprint,
    )
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
    if session_key is not None and session_key in _RUNTIME_LIB_VERIFIED:
        return True
    lock_target = target_triple or "native"
    lock_name = f"runtime.{cargo_profile}.{lock_target}"
    with _build_lock(project_root, lock_name):
        if stored_fingerprint is None:
            stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
        if _runtime_artifact_fingerprint_matches(
            runtime_lib,
            fingerprint,
            fingerprint_path,
            require_artifact_digest=True,
        ):
            if session_key is not None:
                _RUNTIME_LIB_VERIFIED.add(session_key)
            return True
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
        if _maybe_hydrate_artifact_from_canonical_target(
            artifact=runtime_lib,
            fingerprint=fingerprint,
            fingerprint_path=fingerprint_path,
            candidate_artifact=canonical_runtime_lib,
            candidate_fingerprint_path=canonical_fingerprint_path,
            require_artifact_digest=True,
        ):
            if session_key is not None:
                _RUNTIME_LIB_VERIFIED.add(session_key)
            return True
        first_build = not runtime_lib.exists()
        if not json_output:
            if first_build:
                print(
                    "Building optimized runtime (first time only)...",
                    file=sys.stderr,
                )
            else:
                print("Runtime sources changed; rebuilding runtime...", file=sys.stderr)
        cmd = ["cargo", "build", "-p", "molt-runtime", "--profile", cargo_profile]
        if stdlib_profile == "micro":
            cmd.append("--no-default-features")
            # Re-enable the stable micro runtime surface plus explicit runtime
            # target features. User imports must not change this Cargo command.
            micro_features = _dedupe_preserve_order(
                list(runtime_features) + builtin_features + ["stdlib_micro"]
            )
            cmd.extend(["--features", ",".join(micro_features)])
        else:
            # For WASM targets, exclude stdlib_ast (rustpython-parser, ~2MB) and
            # stdlib_unicode_names (unicode_names2, ~1MB) - not useful on WASM
            # and they inflate the binary well past the 3MB Cloudflare free tier.
            is_wasm = target_triple and "wasm" in target_triple
            if is_wasm:
                cmd.append("--no-default-features")
                wasm_features = list(runtime_features) + [
                    "stdlib_crypto",
                    "stdlib_compression",
                    "stdlib_serialization",
                    "stdlib_archive",
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
                    list(runtime_features) + ["stdlib_full"]
                )
                cmd.extend(["--features", ",".join(full_features)])
        if target_triple:
            cmd.extend(["--target", target_triple])
        build_env = _cargo_build_env()
        # Per-session build isolation: route cargo output to
        # target/sessions/<id>/ under the canonical target root
        # when MOLT_SESSION_ID is active to prevent concurrent agents from
        # clobbering each other's runtime artifacts.
        build_env["CARGO_TARGET_DIR"] = str(_cargo_target_root(project_root))
        _maybe_enable_sccache(build_env)
        try:
            with _build_slot() as _slot:
                build = _run_cargo_with_sccache_retry(
                    cmd,
                    cwd=project_root,
                    env=build_env,
                    timeout=cargo_timeout,
                    json_output=json_output,
                    label="Runtime build",
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
    return True
