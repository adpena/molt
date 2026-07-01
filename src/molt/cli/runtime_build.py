from __future__ import annotations

import contextlib
import functools
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import uuid
from concurrent.futures import ThreadPoolExecutor
from typing import Collection, Literal, Mapping, Sequence

from molt._wasm_runtime_exports import wasm_runtime_export_link_args
from molt.cli.artifact_state import (
    _artifact_state_path_for_build_state_root,
    _build_state_root,
    _canonical_build_state_root,
    _canonical_target_root,
    _maybe_hydrate_artifact_from_canonical_target,
    _runtime_fingerprint_path,
    _runtime_target_fingerprint_path,
)
from molt.cli.atomic_io import _atomic_copy_file, _atomic_write_text
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
from molt.cli.runtime_features import (
    _runtime_builtin_features_for_profile,
    _runtime_cargo_features,
    _wasm_runtime_feature_plan,
    runtime_cargo_feature_for_profile,
    runtime_stdlib_profile_for_required_features,
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
from molt.cli.runtime_wasm_validation import (
    _is_valid_runtime_wasm_artifact,
    _is_valid_shared_runtime_wasm_artifact,
    _runtime_wasm_exports_satisfy,
    _runtime_wasm_missing_exports,
    _write_runtime_wasm_integrity_sidecar,
)
from molt.cli import wasm_toolchain
from molt.cli.models import BuildProfile, _RuntimeArtifactState
from molt.wasm_artifact import inspect_wasm_binary as _inspect_wasm_binary


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
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: Collection[str] | None = None,
    extra_runtime_features: Sequence[str] | None = None,
) -> bool:
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
    fingerprint_features: tuple[str, ...]
    if concrete_stdlib_profile == "full":
        fingerprint_features = tuple(
            _dedupe_preserve_order(
                list(runtime_features) + [concrete_stdlib_feature, "default-features"]
            )
        )
    else:
        fingerprint_features = tuple(
            _dedupe_preserve_order(
                list(runtime_features)
                + sorted(builtin_features)
                + [concrete_stdlib_feature, "no-default-features"]
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
        if concrete_stdlib_profile != "full":
            cmd.append("--no-default-features")
            # Re-enable the selected concrete runtime tier plus explicit runtime
            # target features. In auto mode, the caller has already resolved the
            # tier from reached link features before artifact selection.
            concrete_features = _dedupe_preserve_order(
                list(runtime_features) + builtin_features + [concrete_stdlib_feature]
            )
            cmd.extend(["--features", ",".join(concrete_features)])
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
                    list(runtime_features) + [concrete_stdlib_feature]
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


@functools.lru_cache(maxsize=32)
def _resolve_wasm_cargo_profile_cached(
    cargo_profile: str,
    override: str,
) -> str:
    if override:
        return override
    if cargo_profile == "release":
        return "wasm-release"
    return cargo_profile


def _resolve_wasm_cargo_profile(cargo_profile: str) -> str:
    """Map cargo profile for WASM targets.

    Uses the explicit ``wasm-release`` profile instead of generic ``release``
    so WASM artifact size/perf policy can move independently from native
    staticlib policy. Override with ``MOLT_WASM_CARGO_PROFILE``.
    """
    return _resolve_wasm_cargo_profile_cached(
        cargo_profile,
        os.environ.get("MOLT_WASM_CARGO_PROFILE", "").strip(),
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
    requested_exports = None if required_exports is None else frozenset(required_exports)
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
    plans: list[tuple[str, bool, Path | None]] = []
    if kind in {"shared", "both"}:
        plans.append(("shared", False, runtime_state.runtime_wasm))
    if kind in {"reloc", "both"}:
        plans.append(("reloc", True, runtime_state.runtime_reloc_wasm))
    for label, reloc, runtime_path in plans:
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
    fingerprint: dict[str, str | None] | None,
) -> tuple[Path, Path] | None:
    for candidate in candidates:
        fingerprint_path = _runtime_target_fingerprint_path(
            build_state_root,
            candidate,
            cargo_profile=cargo_profile,
            target_label=target_label,
        )
        if _runtime_artifact_fingerprint_matches(
            candidate,
            fingerprint,
            fingerprint_path,
            require_artifact_digest=True,
        ):
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


def _append_rustflags_text(base: str, flags: str) -> str:
    return f"{base.strip()} {flags.strip()}".strip()


def _wasm_link_args_from_rustflags(flags: str) -> list[str]:
    tokens = flags.split()
    link_args: list[str] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token == "-C" and index + 1 < len(tokens):
            value = tokens[index + 1]
            if value.startswith("link-arg="):
                link_args.append(value.removeprefix("link-arg="))
                index += 2
                continue
        if token.startswith("-Clink-arg="):
            link_args.append(token.removeprefix("-Clink-arg="))
        index += 1
    return link_args


def _write_wasm_link_args_response_file(
    response_root: Path,
    *,
    label: str,
    link_args: Sequence[str],
) -> Path:
    digest = hashlib.sha256("\0".join(link_args).encode("utf-8")).hexdigest()
    safe_label = re.sub(r"[^A-Za-z0-9_.-]+", "_", label).strip("._-") or "runtime"
    response_path = response_root / f"{safe_label}.{digest}.rsp"
    _atomic_write_text(response_path, "\n".join(link_args) + "\n")
    return response_path.resolve(strict=False)


def _wasm_link_args_response_rustflags(
    project_root: Path,
    *,
    label: str,
    link_flags: str,
) -> str:
    link_args = _wasm_link_args_from_rustflags(link_flags)
    if not link_args:
        return ""
    response_path = _write_wasm_link_args_response_file(
        _build_state_root(project_root) / "wasm_link_args",
        label=label,
        link_args=link_args,
    )
    return f"-C link-arg=@{response_path}"


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


def _link_runtime_staticlib_to_reloc_wasm(
    *,
    staticlib_path: Path,
    output_path: Path,
    json_output: bool,
    link_timeout: float | None,
    export_link_args: str = "",
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
                "--whole-archive",
                str(staticlib_path),
                "--no-whole-archive",
                str(libc_archive),
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
    effective_stdlib_profile = stdlib_profile or DEFAULT_RUNTIME_STDLIB_PROFILE

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
            return runtime_valid and (
                not validate_exports
                or _runtime_wasm_exports_satisfy(runtime_wasm, required_exports)
            )
    requested_cargo_profile = cargo_profile
    cargo_profile = _resolve_wasm_cargo_profile(cargo_profile)
    profile_dir = _cargo_profile_dir(cargo_profile)
    env = _cargo_build_env()
    runtime_exports = (
        wasm_runtime_export_link_args(
            required_exports,
            resolved_modules=resolved_modules,
        )
        if reloc
        else wasm_runtime_export_link_args()
    )
    if reloc:
        link_flags = runtime_exports
        cargo_link_flags = _wasm_link_args_response_rustflags(
            root,
            label=f"runtime.{_resolve_wasm_cargo_profile(cargo_profile)}.reloc",
            link_flags=link_flags,
        )
    else:
        # Shared-runtime ABI: import the host-provided memory and table, and
        # allow the table to grow for app-specific call_indirect slots.
        shared_import_flags = (
            "-C link-arg=--import-memory -C link-arg=--import-table"
            " -C link-arg=--growable-table"
        )
        # Split-runtime size policy (feedback_wasm_export_treeshaking: "only
        # export table refs for split-runtime builds").  --export-dynamic exports
        # every defined Rust symbol - thousands of mangled
        # serde_json/num_bigint/alloc/core internals.  Those leaked exports (a)
        # bloat the export-name section by ~800KB of mangled strings and (b) pin
        # internal-only functions as wasm-opt GC roots, blocking
        # --remove-unused-module-elements from stripping ~MBs of dead code. The
        # public surface is fully described by the explicit
        # wasm_runtime_export_link_args() allowlist plus the post-link
        # table-ref export pass (_export_wasm_table_refs), so
        # --export-dynamic is pure bloat here.
        link_flags = f"{shared_import_flags}{runtime_exports}"
        cargo_link_flags = _wasm_link_args_response_rustflags(
            root,
            label=f"runtime.{_resolve_wasm_cargo_profile(cargo_profile)}.shared",
            link_flags=link_flags,
        )
    base_rustflags = env.get("RUSTFLAGS", "").strip()
    cargo_rustflags = _append_rustflags_text(base_rustflags, cargo_link_flags)
    fingerprint_rustflags = _append_rustflags_text(base_rustflags, link_flags)
    cargo_rustflags = _wasm_runtime_codegen_rustflags(
        cargo_rustflags,
        simd_enabled=simd_enabled,
        freestanding=freestanding,
    )
    fingerprint_rustflags = _wasm_runtime_codegen_rustflags(
        fingerprint_rustflags,
        simd_enabled=simd_enabled,
        freestanding=freestanding,
    )
    cargo_runtime_features = tuple(["wasm_freestanding"] if freestanding else [])
    builtin_features = _runtime_builtin_features_for_profile(
        effective_stdlib_profile,
        target_triple="wasm32-wasip1",
    )
    runtime_features = cargo_runtime_features
    no_default_features, wasm_cargo_features, fingerprint_features = (
        _wasm_runtime_feature_plan(
            stdlib_profile=effective_stdlib_profile,
            runtime_features=runtime_features,
            builtin_features=builtin_features,
            resolved_modules=resolved_modules,
            required_link_features=required_link_features,
        )
    )
    fingerprint_path = _runtime_fingerprint_path(
        root, runtime_wasm, cargo_profile, "wasm32-wasip1"
    )
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
    if fingerprint is None:
        if not json_output:
            print("Failed to compute runtime wasm fingerprint.", file=sys.stderr)
        return False
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
            and (
                not validate_exports
                or _runtime_wasm_exports_satisfy(
                    target_runtime_wasm,
                    required_exports,
                )
            )
        ):
            assert fingerprint is not None
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
            try:
                _write_runtime_wasm_integrity_sidecar(runtime_wasm)
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
            if not _link_runtime_staticlib_to_reloc_wasm(
                staticlib_path=target_runtime_staticlib,
                output_path=runtime_wasm,
                json_output=json_output,
                link_timeout=cargo_timeout,
                export_link_args=runtime_exports,
            ):
                return False
            try:
                _write_runtime_wasm_integrity_sidecar(runtime_wasm)
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
                or _runtime_wasm_exports_satisfy(runtime_wasm, required_exports)
            )
        ):
            assert fingerprint is not None
            try:
                _write_runtime_wasm_integrity_sidecar(runtime_wasm)
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
            and not _runtime_wasm_exports_satisfy(runtime_wasm, required_exports)
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
        if not json_output:
            print("Runtime sources changed; rebuilding runtime...", file=sys.stderr)
        if cargo_rustflags:
            env["RUSTFLAGS"] = cargo_rustflags
        if os.environ.get("MOLT_WASM_FORCE_CC") == "1":
            _configure_wasm_cc_env(env)
        _configure_wasi_sysroot_env(env)
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
        try:
            build, src = _run_runtime_wasm_cargo_build(
                cmd=cmd,
                root=root,
                env=env,
                cargo_timeout=cargo_timeout,
                profile_dir=profile_dir,
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
        if reloc:
            if not src.exists():
                if not json_output:
                    print(
                        "Runtime wasm build succeeded but staticlib artifact is missing.",
                        file=sys.stderr,
                    )
                return False
            if not _link_runtime_staticlib_to_reloc_wasm(
                staticlib_path=src,
                output_path=runtime_wasm,
                json_output=json_output,
                link_timeout=cargo_timeout,
                export_link_args=runtime_exports,
            ):
                return False
            try:
                _write_runtime_wasm_integrity_sidecar(runtime_wasm)
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
        missing_exports = _runtime_wasm_missing_exports(src, required_exports)
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
        try:
            _write_runtime_wasm_integrity_sidecar(runtime_wasm)
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
            except OSError:
                if not json_output:
                    print(
                        "Warning: failed to write runtime fingerprint metadata.",
                        file=sys.stderr,
                    )
    return True
