from __future__ import annotations

import contextlib
import functools
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from typing import Any, Collection, Mapping, Sequence, cast
import uuid

from molt import backend_daemon_custody as _daemon_custody
from molt.cli.backend_cache import (
    _native_artifact_source_key,
    _shared_stdlib_cache_matches_key_locked,
)
from molt.cli.backend_daemon_logs import (
    _backend_daemon_log_tail,
    _rotate_backend_daemon_log_if_large,
)
from molt.cli.backend_daemon_paths import (
    _backend_daemon_paths as _backend_daemon_paths_bundle,
    _backend_daemon_socket_path_error,
    _short_backend_daemon_socket_dir as _short_backend_daemon_socket_dir_impl,
    _unix_socket_path_exceeds_limit,
)
from molt.cli.backend_daemon_startup import _backend_daemon_spawn_probe_timeout
from molt.cli.cache_fingerprints import _cache_fingerprint, _cache_tooling_fingerprint
from molt.cli.cache_keys import _json_ir_default
from molt.cli.cargo_profiles import _active_artifact_profile_dirs
from molt.cli.command_runtime import _load_cli_harness_memory_guard
from molt.cli.config_resolution import ENTRY_OVERRIDE_ENV
from molt.cli.env_paths import _resolve_env_path
from molt.cli.models import _BackendDaemonCompileResult
from molt.cli.runtime_paths import (
    _build_state_root,
    _cargo_profile_dir,
    _cargo_target_root,
    _cargo_target_root_cached,
    _molt_session_id,
    _runtime_lib_archive_names,
)


_BACKEND_DAEMON_PROTOCOL_VERSION = 1


_BACKEND_CODEGEN_ENV_DIGEST_SCHEMA_VERSION = 4


_DAEMON_CONFIG_DIGEST_SCHEMA_VERSION = 3


_BACKEND_CODEGEN_REQUEST_ENV_KNOBS = (
    "MOLT_DISABLE_DEAD_FUNC_ELIM",
    "MOLT_BACKEND_BATCH_SIZE",
    "MOLT_BACKEND_BATCH_OP_BUDGET",
    "MOLT_MAX_FUNCTION_OPS",
    "MOLT_DISABLE_RC_COALESCING",
    "TIR_DUMP",
    "TIR_OPT_STATS",
    "MOLT_DUMP_CLIF",
    "MOLT_DUMP_CLIF_ON_ERROR",
    "MOLT_DUMP_CLIF_ON_CFG_ERROR",
    "MOLT_DUMP_CLIF_FUNC",
    "MOLT_DUMP_CLIF_FILE",
    "MOLT_DUMP_CLIF_FILE_FILTER",
    "MOLT_DUMP_FINAL_FUNC_IR",
    "MOLT_DUMP_IR",
    # Optimization-pass instruments (mirrors the backend's
    # DAEMON_REQUEST_ENV_KEYS — an instrument is useless if the CLI strips
    # its env key before the daemon sees it).
    "MOLT_DEBUG_ARTIFACT_DIR",
    "MOLT_OVERFLOW_PEEL_STATS",
    "MOLT_PROMOTE_DEBUG",
    "MOLT_INLINE_STATS",
    "MOLT_VERIFY_ANALYSIS",
    "MOLT_DEBUG_BIND",
    "MOLT_BACKEND",
    "MOLT_DEBUG_CHECK_EXC",
    "MOLT_DEBUG_CHECK_EXCEPTION",
    "MOLT_LLVM_DUMP_IR",
    "MOLT_BACKEND_TIMING",
    "MOLT_MEMGVN_REPORT",
    "MOLT_MEMGVN_REPORT_BASELINE",
    "MOLT_MEMGVN_DIAG",
    "MOLT_MEMGVN_DUMP",
    "MOLT_DEBUG_DROP",
    "MOLT_DEBUG_LOWER_FUNC",
    "MOLT_TIR_DUMP",
)


_BACKEND_RESOURCE_ENV_KNOBS = (
    "MOLT_BACKEND_MEMORY_AVAILABLE_GB",
    "MOLT_CLI_MEMORY_AVAILABLE_GB",
    "MOLT_CLI_MEM_AVAILABLE_GB",
    "MOLT_MEMORY_AVAILABLE_GB",
    "MOLT_MEM_AVAILABLE_GB",
    "MOLT_BACKEND_MAX_RSS_GB",
    "MOLT_BACKEND_MEMORY_RESERVE_GB",
    "MOLT_CLI_MEMORY_RESERVE_GB",
    "MOLT_CLI_MEM_RESERVE_GB",
    "MOLT_MEMORY_RESERVE_GB",
    "MOLT_MEM_RESERVE_GB",
    "RAYON_NUM_THREADS",
)


_BACKEND_REQUEST_ENV_KNOBS = (
    _BACKEND_CODEGEN_REQUEST_ENV_KNOBS + _BACKEND_RESOURCE_ENV_KNOBS
)


_NATIVE_CODEGEN_ENV_KNOBS = _BACKEND_CODEGEN_REQUEST_ENV_KNOBS + (
    "MOLT_BACKEND_OPT_LEVEL",
    "MOLT_BACKEND_REGALLOC_ALGORITHM",
    "MOLT_BACKEND_MIN_FUNCTION_ALIGNMENT_LOG2",
    "MOLT_BACKEND_LIBCALL_CALL_CONV",
    "MOLT_BACKEND_ENABLE_VERIFIER",
    "MOLT_DISABLE_STRUCT_ELIDE",
    "MOLT_PORTABLE",
)


_WASM_CODEGEN_ENV_KNOBS = (
    "MOLT_WASM_DATA_BASE",
    "MOLT_WASM_EXTRA_REQUIRED_IMPORTS",
    "MOLT_WASM_MIN_PAGES",
    "MOLT_WASM_LINK",
    "MOLT_WASM_SPLIT_RUNTIME_RUNTIME_TABLE_MIN",
    "MOLT_WASM_TABLE_BASE",
)


_NATIVE_RELOCATABLE_LINKER_ENV_KEYS = ("MOLT_LINKER", "LD", "CC")


_DEFAULT_BACKEND_FEATURES: tuple[str, ...] = ("native-backend",)


@functools.lru_cache(maxsize=256)
def _backend_bin_path_cached(
    project_root_str: str,
    cargo_profile: str,
    cargo_target_override: str | None,
    cwd_str: str,
    os_name: str,
    backend_features: tuple[str, ...] = _DEFAULT_BACKEND_FEATURES,
) -> Path:
    profile_dir = _cargo_profile_dir(cargo_profile)
    target_root = _cargo_target_root_cached(
        project_root_str,
        cargo_target_override,
        cwd_str,
        _molt_session_id(),
    )
    exe_suffix = ".exe" if os_name == "nt" else ""
    # Disambiguate binary path by feature set to prevent native/wasm/rust
    # backend builds from overwriting each other's artifacts.
    if backend_features != _DEFAULT_BACKEND_FEATURES:
        features_tag = "_".join(sorted(backend_features)).replace("-", "_")
        return target_root / profile_dir / f"molt-backend.{features_tag}{exe_suffix}"
    return target_root / profile_dir / f"molt-backend{exe_suffix}"


def _backend_bin_path(
    project_root: Path,
    cargo_profile: str,
    backend_features: tuple[str, ...] = _DEFAULT_BACKEND_FEATURES,
) -> Path:
    return _backend_bin_path_cached(
        os.fspath(project_root),
        cargo_profile,
        os.environ.get("CARGO_TARGET_DIR"),
        os.fspath(Path.cwd()),
        os.name,
        backend_features,
    )


def _backend_features_for_target(
    *,
    is_wasm: bool,
    is_luau_transpile: bool,
    is_rust_transpile: bool,
    env: Mapping[str, str] | None = None,
) -> tuple[str, ...]:
    """Resolve the cargo feature set the backend binary is built with.

    Single source of truth for the target -> backend-feature mapping: the build
    dispatch path selects features from these booleans, and the cache-key
    binary-identity resolver reuses the same mapping so the key tracks the exact
    binary the daemon will run. The ``MOLT_BACKEND == "llvm"`` branch adds the
    ``llvm`` feature, which changes both the binary's codegen and its on-disk
    path (feature-tagged), so it must participate in the identity.
    """
    source = env if env is not None else os.environ
    if is_luau_transpile:
        features: tuple[str, ...] = ("luau-backend",)
    elif is_rust_transpile:
        features = ("rust-backend",)
    elif is_wasm:
        features = ("wasm-backend",)
    else:
        features = ("native-backend",)
    if source.get("MOLT_BACKEND") == "llvm":
        features = (*features, "llvm")
    return features


def _backend_features_for_build_target(
    *,
    target: str,
    is_wasm: bool,
    env: Mapping[str, str] | None = None,
) -> tuple[str, ...]:
    """Resolve backend features from the public ``target`` string.

    ``is_luau_transpile``/``is_rust_transpile`` are pure functions of ``target``
    (``target == "luau"`` / ``target in {"rust", "luau"}``); this convenience
    wrapper lets call sites that only carry ``target`` reach the same single
    source of truth as the build-dispatch booleans.
    """
    return _backend_features_for_target(
        is_wasm=is_wasm,
        is_luau_transpile=target == "luau",
        is_rust_transpile=target in {"rust", "luau"},
        env=env,
    )


def _backend_binary_identity(backend_bin: Path) -> str:
    """Return a stable identity string for the backend binary the daemon runs.

    Finding #4 (design 20 section 4.1) confound: the ``stdlib_shared_<key>.o``
    cache -- and the module/function ``.o`` caches that share
    ``_build_cache_variant`` -- were keyed on the backend *source tree*
    fingerprint (``_cache_fingerprint``) but NOT on the backend *binary*
    identity. A rebuilt backend with different codegen (e.g. drop passes wired
    in) whose source fingerprint happened to be stable -- or any A/B toggle that
    did not monotonically bump tracked source mtimes -- silently linked stale
    objects compiled by the OLD binary. Backend binary identity is therefore
    part of the cache variant itself; shared cache validation only accepts exact
    key/manifest sidecars and never relies on broad mtime sweeps.

    This mirrors the established staleness convention the codebase already uses
    for the per-function TIR cache (``backend_cache_dir_for`` salts its namespace
    with the executable path + mtime) and the intrinsic-symbol sidecar
    (size + mtime): the identity is the resolved path plus ``(mtime_ns, size)``.
    We intentionally use a stat-based stamp rather than a content hash -- hashing
    the multi-hundred-MB binary on every build would dominate cold-start cost,
    and the per-function TIR cache already accepts exactly this trade-off.

    Fail-safe: if the binary cannot be stat'd (not yet built), return a
    ``missing:`` sentinel keyed on the path. That sentinel differs from every
    real-binary identity, so a stale ``.o`` left by a prior binary is never
    reused; once the binary exists the identity becomes its real stamp.
    """
    try:
        resolved = backend_bin.resolve()
    except OSError:
        resolved = backend_bin
    try:
        stat = backend_bin.stat()
    except OSError:
        return f"missing:{resolved}"
    return f"{resolved}|{stat.st_mtime_ns}|{stat.st_size}"


def _runtime_lib_freshness_candidates(
    target_root: Path,
    *,
    target_triple: str | None = None,
    profile_dirs: tuple[str, ...] | None = None,
) -> tuple[Path, ...]:
    profile_dirs = profile_dirs or _active_artifact_profile_dirs()
    names = _runtime_lib_archive_names(target_triple)
    candidates: list[Path] = []
    seen: set[Path] = set()
    for profile_dir in profile_dirs:
        roots = [target_root / profile_dir]
        if target_triple:
            roots.append(target_root / target_triple / profile_dir)
        for root in roots:
            for name in names:
                path = root / name
                if path in seen:
                    continue
                seen.add(path)
                candidates.append(path)
    return tuple(candidates)


@functools.lru_cache(maxsize=64)
def _backend_codegen_env_inputs_cached(
    is_wasm: bool,
    native_values: tuple[tuple[str, str], ...],
    wasm_values: tuple[tuple[str, str], ...],
) -> dict[str, str]:
    payload = {key: value for key, value in native_values}
    if is_wasm:
        payload.update({key: value for key, value in wasm_values})
    return {name: payload[name] for name in sorted(payload)}


def _backend_codegen_env_inputs(
    *,
    is_wasm: bool,
    env: Mapping[str, str] | None = None,
) -> dict[str, str]:
    source = env if env is not None else os.environ
    native_values = tuple(
        (key, value)
        for key in _NATIVE_CODEGEN_ENV_KNOBS
        if (value := (source.get(key) or "").strip())
    )
    wasm_values = tuple(
        (key, value)
        for key in _WASM_CODEGEN_ENV_KNOBS
        if (value := (source.get(key) or "").strip())
    )
    if env is None:
        return _backend_codegen_env_inputs_cached(
            is_wasm,
            native_values,
            wasm_values,
        )
    payload = {key: value for key, value in native_values}
    if is_wasm:
        payload.update({key: value for key, value in wasm_values})
    return {name: payload[name] for name in sorted(payload)}


def _native_relocatable_linker_selection(
    env: Mapping[str, str] | None = None,
) -> tuple[str, str]:
    source = env if env is not None else os.environ
    for key in _NATIVE_RELOCATABLE_LINKER_ENV_KEYS:
        value = (source.get(key) or "").strip()
        if value:
            return key, value
    return "default", "ld"


def _command_has_path_separator(command: str) -> bool:
    return os.sep in command or (os.altsep is not None and os.altsep in command)


def _native_relocatable_linker_identity(
    env: Mapping[str, str] | None = None,
) -> dict[str, object]:
    source = env if env is not None else os.environ
    selected_from, command = _native_relocatable_linker_selection(env)
    path_env = source.get("PATH")
    payload: dict[str, object] = {
        "schema": "native-relocatable-linker-v1",
        "selected_from": selected_from,
        "command": command,
    }
    if _command_has_path_separator(command):
        resolved_path = Path(command)
    else:
        resolved = shutil.which(command, path=path_env)
        if resolved is None:
            payload["search_path_sha256"] = hashlib.sha256(
                (path_env or "").encode("utf-8")
            ).hexdigest()
            resolved_path = Path(command)
        else:
            resolved_path = Path(resolved)
    payload["binary"] = _path_freshness_fingerprint(resolved_path)
    return payload


def _backend_codegen_env_digest(
    *,
    is_wasm: bool,
    env: Mapping[str, str] | None = None,
) -> str:
    payload: dict[str, object] = {
        "schema": _BACKEND_CODEGEN_ENV_DIGEST_SCHEMA_VERSION,
        "target": "wasm" if is_wasm else "native",
        "inputs": _backend_codegen_env_inputs(is_wasm=is_wasm, env=env),
    }
    if not is_wasm:
        payload["native_relocatable_linker"] = _native_relocatable_linker_identity(env)
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _backend_daemon_config_digest(
    project_root: Path,
    cargo_profile: str,
    *,
    env: Mapping[str, str] | None = None,
    backend_bin: Path | None = None,
    target_triple: str | None = None,
) -> str:
    payload = {
        "schema": _DAEMON_CONFIG_DIGEST_SCHEMA_VERSION,
        "project_root": str(project_root.resolve()),
        "cargo_profile": cargo_profile,
        "codegen": _backend_codegen_env_inputs(is_wasm=False, env=env),
        "native_relocatable_linker": _native_relocatable_linker_identity(env),
        "compiler_runtime_backend_fingerprint": _cache_fingerprint(),
        "frontend_tooling_fingerprint": _cache_tooling_fingerprint(),
    }
    if backend_bin is not None:
        payload["freshness"] = _backend_daemon_freshness_inputs(
            project_root,
            backend_bin,
            target_triple=target_triple,
        )
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _path_freshness_fingerprint(path: Path) -> dict[str, object]:
    try:
        stat = path.stat()
    except OSError:
        return {
            "path": os.fspath(path),
            "exists": False,
        }
    return {
        "path": os.fspath(path),
        "exists": True,
        "mtime_ns": stat.st_mtime_ns,
        "size": stat.st_size,
    }


def _source_tree_freshness_fingerprint(root: Path, pattern: str) -> dict[str, object]:
    newest_path: str | None = None
    newest_mtime_ns = 0
    file_count = 0
    try:
        if not root.is_dir():
            return {
                "root": os.fspath(root),
                "exists": False,
                "pattern": pattern,
                "file_count": 0,
                "newest_mtime_ns": 0,
                "newest_path": None,
            }
        for path in root.rglob(pattern):
            try:
                if not path.is_file():
                    continue
                stat = path.stat()
            except OSError:
                continue
            file_count += 1
            if stat.st_mtime_ns > newest_mtime_ns:
                newest_mtime_ns = stat.st_mtime_ns
                newest_path = os.fspath(path)
    except OSError:
        return {
            "root": os.fspath(root),
            "exists": False,
            "pattern": pattern,
            "file_count": file_count,
            "newest_mtime_ns": newest_mtime_ns,
            "newest_path": newest_path,
        }
    return {
        "root": os.fspath(root),
        "exists": True,
        "pattern": pattern,
        "file_count": file_count,
        "newest_mtime_ns": newest_mtime_ns,
        "newest_path": newest_path,
    }


def _backend_daemon_freshness_inputs(
    project_root: Path,
    backend_bin: Path,
    *,
    target_triple: str | None = None,
) -> dict[str, object]:
    target_root = _cargo_target_root(project_root)
    runtime_candidates = [
        _path_freshness_fingerprint(path)
        for path in _runtime_lib_freshness_candidates(
            target_root,
            target_triple=target_triple,
        )
        if path.exists()
    ]
    return {
        "backend_bin": _path_freshness_fingerprint(backend_bin),
        "target_root": os.fspath(target_root),
        "target_triple": target_triple,
        "runtime_libs": runtime_candidates,
        "frontend_init": _path_freshness_fingerprint(
            project_root / "src" / "molt" / "frontend" / "__init__.py"
        ),
        "backend_rs": _source_tree_freshness_fingerprint(
            project_root / "runtime" / "molt-backend" / "src",
            "*.rs",
        ),
        "runtime_rs": _source_tree_freshness_fingerprint(
            project_root / "runtime" / "molt-runtime" / "src",
            "*.rs",
        ),
    }


def _short_backend_daemon_socket_dir(default_dir: Path) -> Path:
    return _short_backend_daemon_socket_dir_impl(
        default_dir,
        path_exceeds_limit=_unix_socket_path_exceeds_limit,
    )


def _backend_daemon_socket_dir(project_root: Path) -> Path:
    # Unix sockets can fail on some external/shared volumes (e.g. exFAT).
    # Keep sockets on a local socket-capable path by default.
    default_dir = _short_backend_daemon_socket_dir(
        Path(tempfile.gettempdir()) / "molt-backend-daemon"
    )
    socket_dir = _resolve_env_path("MOLT_BACKEND_DAEMON_SOCKET_DIR", default_dir)
    socket_dir.mkdir(parents=True, exist_ok=True)
    return socket_dir


@functools.lru_cache(maxsize=256)
def _backend_daemon_paths_cached(
    project_root_str: str,
    cargo_profile: str,
    config_digest: str | None,
    explicit_socket: str,
    socket_dir_override: str | None,
    build_state_root_str: str,
    tempdir_str: str,
    session_id: str = "",  # Must be in cache key for session isolation
) -> tuple[Path, Path, Path]:
    project_root = Path(project_root_str)
    daemon_digest = config_digest or _backend_daemon_config_digest(
        project_root,
        cargo_profile,
    )
    return _backend_daemon_paths_bundle(
        project_root_str=project_root_str,
        cargo_profile=cargo_profile,
        config_digest=daemon_digest,
        explicit_socket=explicit_socket,
        socket_dir_override=socket_dir_override,
        build_state_root_str=build_state_root_str,
        tempdir_str=tempdir_str,
        session_id=session_id,
        cwd=Path.cwd(),
        path_exceeds_limit=_unix_socket_path_exceeds_limit,
    )


def _backend_daemon_socket_path(
    project_root: Path,
    cargo_profile: str,
    *,
    config_digest: str | None = None,
) -> Path:
    socket_path, _log_path, _pid_path = _backend_daemon_paths_cached(
        os.fspath(project_root),
        cargo_profile,
        config_digest,
        os.environ.get("MOLT_BACKEND_DAEMON_SOCKET", "").strip(),
        os.environ.get("MOLT_BACKEND_DAEMON_SOCKET_DIR"),
        os.fspath(_build_state_root(project_root)),
        tempfile.gettempdir(),
        session_id=_molt_session_id(),
    )
    socket_path.parent.mkdir(parents=True, exist_ok=True)
    return socket_path


def _backend_daemon_log_path(
    project_root: Path,
    cargo_profile: str,
    *,
    config_digest: str | None = None,
) -> Path:
    _socket_path, log_path, _pid_path = _backend_daemon_paths_cached(
        os.fspath(project_root),
        cargo_profile,
        config_digest,
        os.environ.get("MOLT_BACKEND_DAEMON_SOCKET", "").strip(),
        os.environ.get("MOLT_BACKEND_DAEMON_SOCKET_DIR"),
        os.fspath(_build_state_root(project_root)),
        tempfile.gettempdir(),
        session_id=_molt_session_id(),
    )
    log_path.parent.mkdir(parents=True, exist_ok=True)
    return log_path


def _backend_daemon_identity_path(
    project_root: Path,
    cargo_profile: str,
    *,
    config_digest: str | None = None,
) -> Path:
    _socket_path, _log_path, identity_path = _backend_daemon_paths_cached(
        os.fspath(project_root),
        cargo_profile,
        config_digest,
        os.environ.get("MOLT_BACKEND_DAEMON_SOCKET", "").strip(),
        os.environ.get("MOLT_BACKEND_DAEMON_SOCKET_DIR"),
        os.fspath(_build_state_root(project_root)),
        tempfile.gettempdir(),
        session_id=_molt_session_id(),
    )
    identity_path.parent.mkdir(parents=True, exist_ok=True)
    return identity_path


_BackendDaemonIdentity = _daemon_custody.BackendDaemonIdentity


def _read_backend_daemon_identity(
    identity_path: Path,
) -> _BackendDaemonIdentity | None:
    return _daemon_custody.read_backend_daemon_identity(identity_path)


def _write_backend_daemon_identity(
    identity_path: Path,
    identity: _BackendDaemonIdentity,
) -> None:
    _daemon_custody.write_backend_daemon_identity(identity_path, identity)


def _remove_backend_daemon_identity(identity_path: Path) -> None:
    _daemon_custody.remove_backend_daemon_identity(identity_path)


def _sweep_orphaned_backend_daemon_locks(
    project_root: Path,
    *,
    include_other_sessions: bool = True,
) -> int:
    """Remove identity files whose recorded daemon process is dead or foreign.

    Returns the number of orphan lock files that were cleaned up. This
    targets the multi-session corruption scenario where an agent session is
    SIGKILLed mid-build and leaves a daemon sidecar behind. Live backend
    daemons are never disturbed; files whose identity does not correspond to
    a live process, is unreadable, or points at a non-daemon process are removed.

    When ``include_other_sessions`` is True, also walks
    ``target/sessions/*/.molt_state/backend_daemon`` so that stale state
    from sibling sessions (e.g. agents that crashed) does not accumulate.
    """
    cleaned = 0

    candidate_roots: list[Path] = []
    own_root = _build_state_root(project_root) / "backend_daemon"
    candidate_roots.append(own_root)

    if include_other_sessions:
        sessions_root = project_root / "target" / "sessions"
        try:
            session_dirs = (
                list(sessions_root.iterdir()) if sessions_root.is_dir() else []
            )
        except OSError:
            session_dirs = []
        for session_dir in session_dirs:
            sibling = session_dir / ".molt_state" / "backend_daemon"
            if sibling == own_root:
                continue
            candidate_roots.append(sibling)

    for daemon_root in candidate_roots:
        try:
            identity_files = list(daemon_root.glob("*.identity.json"))
            legacy_pid_files = list(daemon_root.glob("*.pid"))
        except OSError:
            continue
        for legacy_pid_file in legacy_pid_files:
            with contextlib.suppress(OSError):
                legacy_pid_file.unlink()
            cleaned += 1
        for identity_file in identity_files:
            identity = _read_backend_daemon_identity(identity_file)
            if identity is None:
                _remove_backend_daemon_identity(identity_file)
                cleaned += 1
                continue
            if _pid_alive(identity.pid):
                if _backend_daemon_identity_process_matches(identity):
                    continue
                _remove_backend_daemon_identity(identity_file)
                cleaned += 1
                continue
            _remove_backend_daemon_identity(identity_file)
            cleaned += 1
            try:
                if identity.socket_path.exists():
                    identity.socket_path.unlink()
            except OSError:
                pass

    return cleaned


_BACKEND_DAEMON_ORPHAN_SWEEP_DONE: set[Path] = set()


def _sweep_orphaned_backend_daemon_locks_once(project_root: Path) -> None:
    """Run the orphan sweep at most once per (process, project_root).

    Cheap when no orphans exist, but we still don't want to walk
    ``target/sessions`` on every compile request. The set is keyed on the
    resolved project root so multi-project test runs still get one sweep
    each.
    """
    try:
        key = project_root.resolve()
    except OSError:
        key = project_root
    if key in _BACKEND_DAEMON_ORPHAN_SWEEP_DONE:
        return
    _BACKEND_DAEMON_ORPHAN_SWEEP_DONE.add(key)
    try:
        _sweep_orphaned_backend_daemon_locks(project_root)
    except Exception:
        # Sweep is best-effort — never block daemon spawn on cleanup errors.
        pass


def _backend_daemon_binary_is_newer(
    backend_bin: Path,
    identity_path: Path,
    *,
    target_triple: str | None = None,
) -> bool:
    """Check if the backend binary OR runtime library is newer than the daemon.

    This prevents the daemon from serving stale compiled code when either
    the backend or runtime has been rebuilt.
    """
    try:
        identity_mtime = identity_path.stat().st_mtime + 1e-6
        if backend_bin.stat().st_mtime > identity_mtime:
            return True
        # Also check if the runtime library was rebuilt. The daemon
        # links compiled output against the runtime, so a stale daemon
        # produces binaries with old runtime behavior.  The staticlib
        # bundles all sub-crates (serial, crypto, compression, math, tk)
        # so checking the linkable runtime aliases covers the entire
        # multi-crate tree without conflating stdlib profile identities.
        #
        # Discover project root from Cargo.toml proximity to backend binary,
        # handling CARGO_TARGET_DIR and non-standard layouts.
        candidate = backend_bin.parent
        for _ in range(5):
            if (candidate / "Cargo.toml").exists():
                break
            candidate = candidate.parent
        else:
            candidate = backend_bin.parent.parent.parent  # fallback
        # Resolve the runtime artifact root through the canonical cargo-target
        # resolver so explicit CARGO_TARGET_DIR always wins over session fallback.
        target_root = _cargo_target_root(candidate)
        for runtime_lib in _runtime_lib_freshness_candidates(
            target_root,
            target_triple=target_triple,
        ):
            try:
                if runtime_lib.stat().st_mtime > identity_mtime:
                    return True
            except OSError:
                continue
        # Also check frontend source — if the compiler itself changed,
        # the daemon's cached compilations may produce different IR.
        frontend_init = candidate / "src" / "molt" / "frontend" / "__init__.py"
        try:
            if frontend_init.stat().st_mtime > identity_mtime:
                return True
        except OSError:
            pass
        # Check backend Rust source files — cargo's incremental compilation
        # may NOT update the binary mtime when it determines the output is
        # equivalent (content hash match).  But if any .rs source in the
        # backend crate is newer than the daemon PID, the daemon may be
        # stale.  This is a defence-in-depth check that catches the case
        # where the developer edits backend source, runs cargo build (which
        # skips the link step due to hash match), and expects the daemon to
        # pick up the change.
        backend_src = candidate / "runtime" / "molt-backend" / "src"
        runtime_src = candidate / "runtime" / "molt-runtime" / "src"
        for src_dir in (backend_src, runtime_src):
            try:
                if not src_dir.is_dir():
                    continue
                # Check the newest .rs file in the source tree.
                newest_rs = max(
                    (f.stat().st_mtime for f in src_dir.rglob("*.rs") if f.is_file()),
                    default=0.0,
                )
                if newest_rs > identity_mtime:
                    return True
            except OSError:
                continue
        return False
    except OSError:
        return False


def _pid_alive(pid: int) -> bool:
    return _daemon_custody._pid_alive(pid)


def _backend_daemon_process_command(pid: int) -> str | None:
    return _daemon_custody._process_command(pid)


def _split_backend_daemon_command(command: str) -> tuple[str, ...]:
    return _daemon_custody._split_command(command)


def _command_executable_matches_backend(
    executable: str,
    backend_bin: Path | None,
) -> bool:
    return _daemon_custody._command_executable_matches_backend(
        executable,
        backend_bin,
    )


def _backend_daemon_command_has_socket(
    tokens: Sequence[str],
    socket_path: Path | None,
) -> bool:
    return _daemon_custody._command_has_socket(tokens, socket_path)


def _backend_daemon_command_matches_identity(
    command: str,
    *,
    backend_bin: Path | None,
    socket_path: Path | None,
) -> bool:
    return _daemon_custody.backend_daemon_command_matches_identity(
        command,
        backend_bin=backend_bin,
        socket_path=socket_path,
    )


def _backend_daemon_identity_process_matches(
    identity: _BackendDaemonIdentity,
) -> bool:
    return _daemon_custody._backend_daemon_identity_process_matches(
        identity,
        process_command=_backend_daemon_process_command,
    )


def _backend_daemon_health_probe(
    socket_path: Path,
    timeout: float | None,
) -> tuple[bool, dict[str, Any] | None]:
    if timeout is None:
        return _backend_daemon_ping_health(socket_path, timeout=None)
    return _backend_daemon_ping_health(socket_path, timeout=timeout)


def _backend_daemon_identity_health_matches(
    identity: _BackendDaemonIdentity,
    *,
    timeout: float = 0.25,
) -> bool:
    return _daemon_custody._backend_daemon_identity_health_matches(
        identity,
        health_probe=_backend_daemon_health_probe,
        timeout=timeout,
    )


def _backend_daemon_identity_is_verified(
    identity: _BackendDaemonIdentity,
    *,
    allow_health_probe: bool,
) -> bool:
    return _daemon_custody.backend_daemon_identity_is_verified(
        identity,
        allow_health_probe=allow_health_probe,
        health_probe=_backend_daemon_health_probe,
        process_command=_backend_daemon_process_command,
        pid_alive=_pid_alive,
    )


def _backend_daemon_identity_matches_context(
    identity: _BackendDaemonIdentity,
    *,
    backend_bin: Path,
    socket_path: Path,
    project_root: Path,
    cargo_profile: str,
    config_digest: str | None,
) -> bool:
    return _daemon_custody.backend_daemon_identity_matches_context(
        identity,
        backend_bin=backend_bin,
        socket_path=socket_path,
        project_root=project_root,
        cargo_profile=cargo_profile,
        config_digest=config_digest,
    )


def _backend_daemon_identity_for_pid(
    pid: int,
    *,
    socket_path: Path,
    project_root: Path,
    cargo_profile: str,
    config_digest: str | None,
    backend_bin: Path,
) -> _BackendDaemonIdentity:
    return _daemon_custody.backend_daemon_identity_for_pid(
        pid=pid,
        socket_path=socket_path,
        project_root=project_root,
        cargo_profile=cargo_profile,
        config_digest=config_digest,
        backend_bin=backend_bin,
        process_command=_backend_daemon_process_command,
    )


def _backend_daemon_identity_from_health(
    health: dict[str, Any] | None,
    *,
    socket_path: Path,
    project_root: Path,
    cargo_profile: str,
    config_digest: str | None,
    backend_bin: Path,
) -> _BackendDaemonIdentity | None:
    return _daemon_custody.backend_daemon_identity_from_health(
        health,
        socket_path=socket_path,
        project_root=project_root,
        cargo_profile=cargo_profile,
        config_digest=config_digest,
        backend_bin=backend_bin,
        process_command=_backend_daemon_process_command,
    )


def _terminate_backend_daemon_identity(
    identity: _BackendDaemonIdentity,
    *,
    grace: float = 1.0,
) -> bool:
    return _daemon_custody.terminate_backend_daemon_identity(
        identity,
        grace=grace,
        health_probe=_backend_daemon_health_probe,
        process_command=_backend_daemon_process_command,
        pid_alive=_pid_alive,
    )


def _backend_daemon_request_bytes(
    socket_path: Path,
    data: bytes,
    *,
    timeout: float | None,
    daemon_identity: _BackendDaemonIdentity | None = None,
    project_root: Path | None = None,
) -> tuple[dict[str, Any] | None, str | None]:
    request_sentinel = None
    if project_root is not None:
        try:
            harness_memory_guard = _load_cli_harness_memory_guard(project_root)
            guard_context = harness_memory_guard.HarnessExecutionContext.from_env(
                "MOLT_BUILD",
                os.environ,
                repo_root=project_root,
            )
            request_sentinel = guard_context.start_repo_sentinel(
                label=(
                    "backend_daemon_request"
                    if daemon_identity is None
                    else f"backend_daemon_request_{daemon_identity.pid}"
                ),
                drain_on_exit=False,
                drain_until_clean_sec=0.0,
                drain_max_runtime_sec=0.0,
            )
        except RuntimeError:
            raise
        except Exception as exc:
            return None, f"backend daemon request memory guard failed: {exc}"
    try:
        try:
            af_unix = getattr(socket, "AF_UNIX", None)
            if af_unix is None:
                return (
                    None,
                    "backend daemon request requires a Python build with "
                    "AF_UNIX socket support",
                )
            with socket.socket(af_unix, socket.SOCK_STREAM) as sock:
                if timeout is not None or daemon_identity is not None:
                    sock.settimeout(timeout if timeout is not None else 1.0)
                sock.connect(str(socket_path))
                return _backend_daemon_request_on_socket(
                    sock,
                    data,
                    socket_path=socket_path,
                    shutdown_write=True,
                    daemon_identity=daemon_identity,
                )
        except OSError as exc:
            return None, f"backend daemon connection failed: {exc}"
    finally:
        if request_sentinel is not None:
            request_sentinel.__exit__(None, None, None)


def _backend_daemon_empty_response_error(
    socket_path: Path,
    daemon_identity: _BackendDaemonIdentity | None,
) -> str:
    base = "backend daemon returned empty response"
    if daemon_identity is None:
        return base
    verified_live = _backend_daemon_identity_is_verified(
        daemon_identity,
        allow_health_probe=False,
    )
    log_path = _backend_daemon_log_path(
        daemon_identity.project_root,
        daemon_identity.cargo_profile,
        config_digest=daemon_identity.config_digest,
    )
    details = (
        f"{base} (pid={daemon_identity.pid}, "
        f"verified_live={str(verified_live).lower()}, "
        f"socket={socket_path}, log={log_path})"
    )
    log_tail = _backend_daemon_log_tail(log_path)
    if log_tail:
        return f"{details}\nLast daemon log lines:\n{log_tail}"
    return f"{details}\n(no daemon log output captured at {log_path})"


def _backend_daemon_request_on_socket(
    sock: socket.socket,
    data: bytes,
    *,
    socket_path: Path | None = None,
    shutdown_write: bool,
    daemon_identity: _BackendDaemonIdentity | None = None,
) -> tuple[dict[str, Any] | None, str | None]:
    try:
        sock.sendall(data)
        if shutdown_write:
            sock.shutdown(socket.SHUT_WR)
        raw = bytearray()
        recv_buffer = bytearray(65536)
        recv_view = memoryview(recv_buffer)
        while True:
            try:
                received = sock.recv_into(recv_view)
            except socket.timeout as exc:
                if daemon_identity is not None:
                    if not _backend_daemon_identity_is_verified(
                        daemon_identity,
                        allow_health_probe=False,
                    ):
                        return None, "backend daemon died while request was in flight"
                    continue
                return None, f"backend daemon connection failed: {exc}"
            if received == 0:
                break
            raw.extend(recv_view[:received])
            if b"\n" in raw:
                raw = raw.partition(b"\n")[0]
                break
    except OSError as exc:
        return None, f"backend daemon connection failed: {exc}"
    if not raw or all(byte in b" \t\r\n" for byte in raw):
        return None, _backend_daemon_empty_response_error(
            socket_path or Path("."),
            daemon_identity,
        )
    try:
        response = json.loads(raw)
    except json.JSONDecodeError as exc:
        return None, f"backend daemon returned invalid JSON: {exc}"
    if not isinstance(response, dict):
        return None, "backend daemon returned non-object response"
    return response, None


def _backend_daemon_request(
    socket_path: Path,
    payload: dict[str, Any],
    *,
    timeout: float | None,
) -> tuple[dict[str, Any] | None, str | None]:
    data, encode_err = _backend_daemon_request_payload_bytes(payload)
    if encode_err is not None:
        return None, encode_err
    assert data is not None
    return _backend_daemon_request_bytes(socket_path, data, timeout=timeout)


def _backend_daemon_request_payload_bytes(
    payload: dict[str, Any],
) -> tuple[bytes | None, str | None]:
    try:
        encoded = json.dumps(
            payload,
            default=_json_ir_default,
            separators=(",", ":"),
        ).encode("utf-8")
    except (TypeError, ValueError) as exc:
        return None, f"backend daemon request encode failed: {exc}"
    return encoded + b"\n", None


def _write_backend_ir_json_file(path: Path, ir: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        json.dump(ir, handle, separators=(",", ":"), default=_json_ir_default)


def _write_backend_ir_lease(project_root: Path, ir: Mapping[str, Any]) -> Path:
    lease_dir = project_root / "tmp" / "backend-ir-leases"
    lease_dir.mkdir(parents=True, exist_ok=True)
    lease_path = lease_dir / f"ir-{os.getpid()}-{uuid.uuid4().hex}.json"
    _write_backend_ir_json_file(lease_path, ir)
    return lease_path


def _write_backend_daemon_ir_lease(project_root: Path, ir: Mapping[str, Any]) -> Path:
    return _write_backend_ir_lease(project_root, ir)


def _backend_daemon_compile_request_bytes(
    *,
    ir: Mapping[str, Any] | None,
    ir_path: Path | None = None,
    backend_output: Path,
    is_wasm: bool,
    wasm_link: bool,
    wasm_data_base: int | None,
    wasm_table_base: int | None,
    wasm_split_runtime_runtime_table_min: int | None = None,
    target_triple: str | None,
    cache_key: str | None,
    function_cache_key: str | None,
    config_digest: str | None,
    skip_module_output_if_synced: bool,
    skip_function_output_if_synced: bool,
    entry_module: str | None = None,
    stdlib_object_path: Path | None = None,
    stdlib_object_cache_key: str | None = None,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols_json: str | None = None,
    probe_cache_only: bool = False,
    include_health: bool = False,
) -> tuple[bytes | None, str | None]:
    effective_cache_key = _native_artifact_source_key(
        cache_key,
        stdlib_object_cache_key=stdlib_object_cache_key,
        is_wasm=is_wasm,
    )
    effective_function_cache_key = _native_artifact_source_key(
        function_cache_key,
        stdlib_object_cache_key=stdlib_object_cache_key,
        is_wasm=is_wasm,
    )
    job: dict[str, Any] = {
        "id": "job0",
        "is_wasm": is_wasm,
        "target_triple": target_triple,
        "wasm_link": wasm_link,
        "wasm_data_base": wasm_data_base,
        "wasm_table_base": wasm_table_base,
        "wasm_split_runtime_runtime_table_min": wasm_split_runtime_runtime_table_min,
        "output": str(backend_output),
        "cache_key": effective_cache_key,
        "function_cache_key": effective_function_cache_key,
        "skip_module_output_if_synced": skip_module_output_if_synced,
        "skip_function_output_if_synced": skip_function_output_if_synced,
    }
    if probe_cache_only:
        job["probe_cache_only"] = True
    elif ir is not None and ir_path is not None:
        return (
            None,
            "backend daemon request must use exactly one IR custody field: "
            "ir or ir_path",
        )
    elif ir_path is not None:
        job["ir_path"] = str(ir_path)
    elif ir is not None:
        job["ir"] = ir
    jobs: list[dict[str, Any]] = [job]
    payload: dict[str, Any] = {
        "version": _BACKEND_DAEMON_PROTOCOL_VERSION,
        "jobs": jobs,
    }
    if config_digest:
        payload["config_digest"] = config_digest
    if include_health:
        payload["include_health"] = True
    # Pass through optimization-relevant env vars so the daemon applies
    # them per-request (the daemon process inherits env from startup,
    # not from the build request).
    env_passthrough = {}
    for key in _BACKEND_REQUEST_ENV_KNOBS:
        val = os.environ.get(key)
        if val is not None:
            env_passthrough[key] = val
    if entry_module:
        env_passthrough[ENTRY_OVERRIDE_ENV] = entry_module
    if stdlib_object_path is not None:
        env_passthrough["MOLT_STDLIB_OBJ"] = str(stdlib_object_path)
    if stdlib_object_cache_key:
        env_passthrough["MOLT_STDLIB_CACHE_KEY"] = stdlib_object_cache_key
    if stdlib_object_manifest:
        env_passthrough["MOLT_STDLIB_CACHE_MANIFEST"] = stdlib_object_manifest
    if stdlib_module_symbols_json:
        env_passthrough["MOLT_STDLIB_MODULE_SYMBOLS"] = stdlib_module_symbols_json
    # Per-app intrinsic resolver validation set: the file of intrinsic symbols the
    # linked runtime staticlib defines. Set in the ambient env once the runtime
    # lib is ready (see `_stage_runtime_intrinsic_symbols_for_native_codegen`);
    # forward it so the daemon's resolver never references an intrinsic absent
    # from the staticlib.
    runtime_intrinsic_symbols = os.environ.get("MOLT_RUNTIME_INTRINSIC_SYMBOLS")
    if runtime_intrinsic_symbols:
        env_passthrough["MOLT_RUNTIME_INTRINSIC_SYMBOLS"] = runtime_intrinsic_symbols
    if env_passthrough:
        payload["env"] = env_passthrough
    return _backend_daemon_request_payload_bytes(payload)


def _backend_daemon_health_from_response(
    response: dict[str, Any],
) -> dict[str, Any] | None:
    raw = response.get("health")
    if not isinstance(raw, dict):
        return None
    health: dict[str, Any] = {}
    int_fields = {
        "protocol_version",
        "pid",
        "uptime_ms",
        "cache_entries",
        "cache_bytes",
        "cache_max_bytes",
        "request_limit_bytes",
        "max_jobs",
        "requests_total",
        "jobs_total",
        "cache_hits",
        "cache_misses",
    }
    for field_name in int_fields:
        value = raw.get(field_name)
        if isinstance(value, int):
            health[field_name] = value
    string_fields = {
        "spawn_config_digest",
        "active_config_digest",
    }
    for field_name in string_fields:
        value = raw.get(field_name)
        if isinstance(value, str) and value:
            health[field_name] = value
    return health or None


def _backend_daemon_text_field(
    payload: Mapping[str, Any],
    field_name: str,
) -> str | None:
    value = payload.get(field_name)
    if not isinstance(value, str):
        return None
    value = value.strip()
    return value or None


def _backend_daemon_job_failure_message(job: Mapping[str, Any]) -> str | None:
    for field_name in ("message", "error"):
        message = _backend_daemon_text_field(job, field_name)
        if message is not None:
            return message
    return None


def _backend_daemon_response_failure_message(
    response: Mapping[str, Any],
    *,
    default: str,
) -> str:
    response_jobs = response.get("jobs")
    if isinstance(response_jobs, list):
        for raw_job in response_jobs:
            if not isinstance(raw_job, dict) or bool(raw_job.get("ok")):
                continue
            message = _backend_daemon_job_failure_message(raw_job)
            if message is not None:
                return message
    for field_name in ("error", "message"):
        message = _backend_daemon_text_field(response, field_name)
        if message is not None:
            return message
    return default


def _backend_daemon_ping_health(
    socket_path: Path, *, timeout: float | None
) -> tuple[bool, dict[str, Any] | None]:
    payload = {"version": _BACKEND_DAEMON_PROTOCOL_VERSION, "ping": True}
    response, err = _backend_daemon_request(socket_path, payload, timeout=timeout)
    if err is not None or response is None:
        return False, None
    health = _backend_daemon_health_from_response(response)
    return bool(response.get("ok")) and bool(response.get("pong")), health


def _backend_daemon_ping(socket_path: Path, *, timeout: float | None) -> bool:
    ready, _ = _backend_daemon_ping_health(socket_path, timeout=timeout)
    return ready


def _backend_daemon_wait_until_ready(
    socket_path: Path,
    *,
    ready_timeout: float | None,
    probe_timeout: float | None = None,
) -> tuple[bool, dict[str, Any] | None]:
    deadline = (
        time.monotonic() + max(0.05, ready_timeout)
        if ready_timeout is not None
        else None
    )
    while deadline is None or time.monotonic() < deadline:
        ready, health = _backend_daemon_ping_health(socket_path, timeout=probe_timeout)
        if ready:
            return True, health
        time.sleep(0.05)
    return False, None


def _backend_daemon_retryable_error(error: str | None) -> bool:
    if not error:
        return False
    lowered = error.lower()
    return (
        "connection failed" in lowered
        or "died while request was in flight" in lowered
        or "empty response" in lowered
        or "invalid json" in lowered
        or "unsupported protocol version" in lowered
        or "missing job results" in lowered
        or "output is missing" in lowered
    )


def _start_backend_daemon(
    backend_bin: Path,
    socket_path: Path,
    *,
    cargo_profile: str,
    project_root: Path,
    target_triple: str | None,
    config_digest: str | None,
    startup_timeout: float | None,
    json_output: bool,
    warnings: list[str],
) -> bool:
    def _report_daemon_issue(message: str) -> None:
        if json_output:
            warnings.append(message)
        else:
            print(message, file=sys.stderr)

    if _unix_socket_path_exceeds_limit(socket_path):
        _report_daemon_issue(_backend_daemon_socket_path_error(socket_path))
        return False
    startup_wait = startup_timeout if startup_timeout is not None else None
    identity_path = _backend_daemon_identity_path(
        project_root,
        cargo_profile,
        config_digest=config_digest,
    )
    log_path = _backend_daemon_log_path(
        project_root,
        cargo_profile,
        config_digest=config_digest,
    )
    # Cheap, idempotent cross-session orphan sweep. Verified live daemons are
    # never disturbed. Suppress for the common case where the current identity
    # is already alive — that path will short-circuit below.
    _sweep_orphaned_backend_daemon_locks_once(project_root)
    existing_identity = _read_backend_daemon_identity(identity_path)
    if existing_identity is not None:
        if not _backend_daemon_identity_matches_context(
            existing_identity,
            backend_bin=backend_bin,
            socket_path=socket_path,
            project_root=project_root,
            cargo_profile=cargo_profile,
            config_digest=config_digest,
        ):
            _report_daemon_issue(
                "Ignoring stale backend daemon identity "
                f"{identity_path}; recorded context no longer matches "
                f"{backend_bin.name} --daemon --socket {socket_path}."
            )
            _remove_backend_daemon_identity(identity_path)
            existing_identity = None
        elif _backend_daemon_identity_is_verified(
            existing_identity,
            allow_health_probe=True,
        ):
            if _backend_daemon_binary_is_newer(
                backend_bin=backend_bin,
                identity_path=identity_path,
                target_triple=target_triple,
            ):
                _report_daemon_issue(
                    "Backend daemon freshness changed for "
                    f"{identity_path}; preserving verified pid "
                    f"{existing_identity.pid} and using one-shot backend compile "
                    "for this request. Production daemon paths must encode the "
                    "backend/runtime/source freshness digest."
                )
                return False
            else:
                if socket_path.exists():
                    probe_window = _backend_daemon_spawn_probe_timeout(startup_wait)
                    ready, _ = _backend_daemon_wait_until_ready(
                        socket_path,
                        ready_timeout=probe_window,
                        probe_timeout=probe_window,
                    )
                    if ready:
                        return True
                    # A verified daemon that misses the short startup probe may
                    # simply be inside a long synchronous compile. Treat the
                    # identity as authoritative and let the compile request
                    # queue on that socket; restarting here can orphan a busy
                    # daemon and submit the same heavy full-IR request twice.
                    return True
                else:
                    _terminate_backend_daemon_identity(
                        existing_identity,
                        grace=1.0,
                    )
                    _remove_backend_daemon_identity(identity_path)
                    existing_identity = None
        else:
            _report_daemon_issue(
                "Ignoring stale backend daemon identity "
                f"{identity_path}; recorded pid {existing_identity.pid} is not "
                "a verified live daemon."
            )
            _remove_backend_daemon_identity(identity_path)
            existing_identity = None
    try:
        if socket_path.exists():
            socket_identity = _read_backend_daemon_identity(identity_path)
            _daemon_alive = socket_identity is not None and (
                _backend_daemon_identity_matches_context(
                    socket_identity,
                    backend_bin=backend_bin,
                    socket_path=socket_path,
                    project_root=project_root,
                    cargo_profile=cargo_profile,
                    config_digest=config_digest,
                )
                and _backend_daemon_identity_is_verified(
                    socket_identity,
                    allow_health_probe=True,
                )
            )
            probe_window = _backend_daemon_spawn_probe_timeout(startup_wait)
            # A live PID is not enough to trust the socket. If the socket
            # cannot answer a short probe, treat it as stale and restart.
            if _daemon_alive:
                ready, _ = _backend_daemon_wait_until_ready(
                    socket_path,
                    ready_timeout=probe_window,
                    probe_timeout=probe_window,
                )
                if ready:
                    return True
                message = (
                    f"Backend daemon socket {socket_path} existed but did not answer "
                    f"readiness probes within {probe_window:.2f}s; "
                    "removing stale socket."
                )
            else:
                # Quick single-shot probe in case an orphaned daemon is
                # still listening (PID file was lost but process lives).
                ready, health = _backend_daemon_wait_until_ready(
                    socket_path,
                    ready_timeout=probe_window,
                    probe_timeout=probe_window,
                )
                if ready:
                    adopted_identity = _backend_daemon_identity_from_health(
                        health,
                        socket_path=socket_path,
                        project_root=project_root,
                        cargo_profile=cargo_profile,
                        config_digest=config_digest,
                        backend_bin=backend_bin,
                    )
                    if adopted_identity is not None:
                        _write_backend_daemon_identity(
                            identity_path,
                            adopted_identity,
                        )
                        return True
                message = (
                    f"Removed stale daemon socket {socket_path.name} "
                    f"(no live daemon process found)."
                )
            log_tail = _backend_daemon_log_tail(log_path)
            if log_tail:
                message = f"{message}\nLast daemon log lines:\n{log_tail}"
            _report_daemon_issue(message)
            socket_path.unlink()
    except OSError:
        pass
    try:
        socket_path.with_suffix(".redirect").unlink()
    except OSError:
        pass
    daemon_pid: int | None = None
    daemon_proc: subprocess.Popen[bytes] | None = None
    daemon_env = dict(os.environ)
    harness_memory_guard = _load_cli_harness_memory_guard(project_root)
    daemon_context = harness_memory_guard.HarnessExecutionContext.from_env(
        "MOLT_BUILD",
        daemon_env,
        repo_root=project_root,
    )
    daemon_env = dict(daemon_context.env)
    if config_digest:
        daemon_env["MOLT_BACKEND_DAEMON_CONFIG_DIGEST"] = config_digest
    daemon_popen_kwargs: dict[str, Any] = {"start_new_session": True}
    daemon_popen_kwargs.update(daemon_context.process_group_kwargs())
    daemon_sentinel = daemon_context.start_repo_sentinel(
        label="backend_daemon_start",
        drain_on_exit=False,
        drain_until_clean_sec=0.0,
        drain_max_runtime_sec=_backend_daemon_spawn_probe_timeout(startup_wait) + 1.0,
    )
    try:
        try:
            log_path.parent.mkdir(parents=True, exist_ok=True)
            # Rotate the log if it has grown beyond the configured cap before the
            # new daemon starts appending. Long-running multi-session repos see
            # daemon logs grow to tens of megabytes otherwise, which slows down
            # post-mortem tail reads and bloats the artifact root.
            _rotate_backend_daemon_log_if_large(log_path)
            # Propagate all environment variables to the daemon so that
            # debug env vars (MOLT_TRACE_EQ, MOLT_DEBUG_EXCEPTION_FLOW etc.)
            # reach the backend process. Daemon stderr goes to the log file
            # so it's always available for post-mortem debugging.
            with log_path.open("ab") as log_file:
                # The daemon is launched in bytes mode (no ``text``/``encoding``
                # arg here or in ``daemon_popen_kwargs``, which only carries OS
                # process-group controls — ``start_new_session`` / ``preexec_fn``
                # / ``creationflags``).  Splatting the ``dict[str, Any]`` of those
                # controls erases Popen's bytes-vs-text overload for the checker,
                # which then infers ``Popen[str]``; cast restores the proven
                # ``Popen[bytes]`` the declared ``daemon_proc`` slot expects.
                daemon_proc = cast(
                    "subprocess.Popen[bytes]",
                    subprocess.Popen(
                        [str(backend_bin), "--daemon", "--socket", str(socket_path)],
                        cwd=project_root,
                        stdout=log_file,
                        stderr=subprocess.STDOUT,
                        env=daemon_env,
                        **daemon_popen_kwargs,
                    ),
                )
                daemon_pid = daemon_proc.pid
                _write_backend_daemon_identity(
                    identity_path,
                    _backend_daemon_identity_for_pid(
                        daemon_pid,
                        socket_path=socket_path,
                        project_root=project_root,
                        cargo_profile=cargo_profile,
                        config_digest=config_digest,
                        backend_bin=backend_bin,
                    ),
                )
        except OSError as exc:
            if daemon_pid is not None:
                _remove_backend_daemon_identity(identity_path)
            if not json_output:
                print(f"Failed to start backend daemon: {exc}", file=sys.stderr)
            return False
        ready, _ = _backend_daemon_wait_until_ready(
            socket_path,
            ready_timeout=_backend_daemon_spawn_probe_timeout(startup_wait),
            probe_timeout=None,
        )
        if ready:
            return True
        probe_window = _backend_daemon_spawn_probe_timeout(startup_wait)
        # Surface concrete subprocess status instead of a bare timeout. If the
        # daemon already exited (crash, missing dynamic dep, port conflict, etc.)
        # the returncode tells us so directly; otherwise we know it is still
        # running but unresponsive within the probe window.
        proc_status = "process status unavailable"
        if daemon_proc is not None:
            exit_code = daemon_proc.poll()
            if exit_code is None:
                proc_status = (
                    f"daemon process pid={daemon_pid} still running but did not "
                    f"answer readiness probes"
                )
            elif exit_code < 0:
                proc_status = (
                    f"daemon process pid={daemon_pid} terminated by signal "
                    f"{-exit_code} before readiness"
                )
            else:
                proc_status = (
                    f"daemon process pid={daemon_pid} exited with code "
                    f"{exit_code} before readiness"
                )
        message = (
            "Backend daemon did not become ready after spawn within "
            f"{probe_window:.2f}s ({proc_status}); falling back to one-shot "
            "compile for this build."
        )
        log_tail = _backend_daemon_log_tail(log_path)
        if log_tail:
            message = f"{message}\nLast daemon log lines:\n{log_tail}"
        else:
            message = f"{message}\n(no daemon log output captured at {log_path})"
        _report_daemon_issue(message)
        return False
    finally:
        if daemon_sentinel is not None:
            daemon_sentinel.__exit__(None, None, None)


def _compile_with_backend_daemon(
    socket_path: Path,
    *,
    project_root: Path,
    ir: Mapping[str, Any],
    backend_output: Path,
    is_wasm: bool,
    wasm_link: bool,
    wasm_data_base: int | None,
    wasm_table_base: int | None,
    wasm_split_runtime_runtime_table_min: int | None = None,
    target_triple: str | None,
    cache_key: str | None,
    function_cache_key: str | None,
    config_digest: str | None,
    skip_module_output_if_synced: bool = False,
    skip_function_output_if_synced: bool = False,
    entry_module: str | None = None,
    stdlib_object_path: Path | None = None,
    stdlib_object_cache_key: str | None = None,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols_json: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    timeout: float | None,
    request_bytes: bytes | None = None,
    daemon_identity: _BackendDaemonIdentity | None = None,
) -> _BackendDaemonCompileResult:
    full_request_bytes = request_bytes
    probe_request_bytes: bytes | None = None
    ir_lease_path: Path | None = None
    cache_probe_allowed = True

    def encode_full_request_bytes() -> tuple[bytes | None, str | None]:
        nonlocal full_request_bytes, ir_lease_path
        if full_request_bytes is not None:
            return full_request_bytes, None
        if ir_lease_path is None:
            try:
                ir_lease_path = _write_backend_daemon_ir_lease(project_root, ir)
            except OSError as exc:
                return None, f"backend daemon IR lease write failed: {exc}"
        full_request_bytes, encode_err = _backend_daemon_compile_request_bytes(
            ir=None,
            ir_path=ir_lease_path,
            backend_output=backend_output,
            is_wasm=is_wasm,
            wasm_link=wasm_link,
            wasm_data_base=wasm_data_base,
            wasm_table_base=wasm_table_base,
            wasm_split_runtime_runtime_table_min=wasm_split_runtime_runtime_table_min,
            target_triple=target_triple,
            cache_key=cache_key,
            function_cache_key=function_cache_key,
            config_digest=config_digest,
            skip_module_output_if_synced=skip_module_output_if_synced,
            skip_function_output_if_synced=skip_function_output_if_synced,
            entry_module=entry_module,
            stdlib_object_path=stdlib_object_path,
            stdlib_object_cache_key=stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols_json=stdlib_module_symbols_json,
            include_health=False,
        )
        return full_request_bytes, encode_err

    def cleanup_ir_lease() -> None:
        nonlocal ir_lease_path
        if ir_lease_path is None:
            return
        with contextlib.suppress(OSError):
            ir_lease_path.unlink()
        ir_lease_path = None

    if not is_wasm and stdlib_object_path is not None:
        cache_probe_allowed = _shared_stdlib_cache_matches_key_locked(
            stdlib_object_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
        )
    if (
        request_bytes is None
        and (cache_key or function_cache_key)
        and cache_probe_allowed
    ):
        probe_request_bytes, probe_encode_err = _backend_daemon_compile_request_bytes(
            ir=None,
            backend_output=backend_output,
            is_wasm=is_wasm,
            wasm_link=wasm_link,
            wasm_data_base=wasm_data_base,
            wasm_table_base=wasm_table_base,
            wasm_split_runtime_runtime_table_min=wasm_split_runtime_runtime_table_min,
            target_triple=target_triple,
            cache_key=cache_key,
            function_cache_key=function_cache_key,
            config_digest=config_digest,
            skip_module_output_if_synced=skip_module_output_if_synced,
            skip_function_output_if_synced=skip_function_output_if_synced,
            entry_module=entry_module,
            stdlib_object_path=stdlib_object_path,
            stdlib_object_cache_key=stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols_json=stdlib_module_symbols_json,
            probe_cache_only=True,
            include_health=False,
        )
        if probe_encode_err is not None:
            return _BackendDaemonCompileResult(
                False, probe_encode_err, None, None, None, True, False
            )
    elif full_request_bytes is None:
        full_request_bytes, encode_err = encode_full_request_bytes()
        if encode_err is not None:
            cleanup_ir_lease()
            return _BackendDaemonCompileResult(
                False, encode_err, None, None, None, True, False
            )
        assert full_request_bytes is not None
    if probe_request_bytes is not None:
        full_request_sent = False
        response, err = _backend_daemon_request_bytes(
            socket_path,
            probe_request_bytes,
            timeout=timeout,
            daemon_identity=daemon_identity,
            project_root=project_root,
        )
    else:
        assert full_request_bytes is not None
        full_request_sent = True
        response, err = _backend_daemon_request_bytes(
            socket_path,
            full_request_bytes,
            timeout=timeout,
            daemon_identity=daemon_identity,
            project_root=project_root,
        )
        cleanup_ir_lease()
    if err is not None:
        return _BackendDaemonCompileResult(
            False, err, None, None, None, True, False, full_request_sent
        )
    if response is None:
        return _BackendDaemonCompileResult(
            False,
            "backend daemon returned no response",
            None,
            None,
            None,
            True,
            False,
            full_request_sent,
        )
    health = _backend_daemon_health_from_response(response)
    if not bool(response.get("ok")):
        return _BackendDaemonCompileResult(
            False,
            _backend_daemon_response_failure_message(
                response,
                default="backend daemon compile request failed",
            ),
            health,
            None,
            None,
            True,
            False,
            full_request_sent,
        )
    response_jobs = response.get("jobs")
    if not isinstance(response_jobs, list) or not response_jobs:
        return _BackendDaemonCompileResult(
            False,
            "backend daemon response missing job results",
            health,
            None,
            None,
            True,
            False,
            full_request_sent,
        )
    first = response_jobs[0]
    if not isinstance(first, dict):
        return _BackendDaemonCompileResult(
            False,
            "backend daemon response had malformed job payload",
            health,
            None,
            None,
            True,
            False,
            full_request_sent,
        )
    cached: bool | None = (
        first.get("cached") if isinstance(first.get("cached"), bool) else None
    )
    raw_tier = first.get("cache_tier")
    cache_tier = (
        raw_tier.strip() if isinstance(raw_tier, str) and raw_tier.strip() else None
    )
    output_written = (
        first.get("output_written")
        if isinstance(first.get("output_written"), bool)
        else True
    )
    needs_ir = bool(first.get("needs_ir"))
    output_exists = not output_written
    if needs_ir and probe_request_bytes is not None:
        if full_request_bytes is None:
            full_request_bytes, encode_err = encode_full_request_bytes()
            if encode_err is not None:
                cleanup_ir_lease()
                return _BackendDaemonCompileResult(
                    False, encode_err, health, None, None, True, False
                )
            assert full_request_bytes is not None
        response, err = _backend_daemon_request_bytes(
            socket_path,
            full_request_bytes,
            timeout=timeout,
            daemon_identity=daemon_identity,
            project_root=project_root,
        )
        full_request_sent = True
        cleanup_ir_lease()
        if err is not None:
            return _BackendDaemonCompileResult(
                False, err, health, None, None, True, False, full_request_sent
            )
        if response is None:
            return _BackendDaemonCompileResult(
                False,
                "backend daemon returned no response",
                health,
                None,
                None,
                True,
                False,
                full_request_sent,
            )
        health = _backend_daemon_health_from_response(response)
        if not bool(response.get("ok")):
            return _BackendDaemonCompileResult(
                False,
                _backend_daemon_response_failure_message(
                    response,
                    default="backend daemon compile request failed",
                ),
                health,
                None,
                None,
                True,
                False,
                full_request_sent,
            )
        response_jobs = response.get("jobs")
        if not isinstance(response_jobs, list) or not response_jobs:
            return _BackendDaemonCompileResult(
                False,
                "backend daemon response missing job results",
                health,
                None,
                None,
                True,
                False,
                full_request_sent,
            )
        first = response_jobs[0]
        if not isinstance(first, dict):
            return _BackendDaemonCompileResult(
                False,
                "backend daemon response had malformed job payload",
                health,
                None,
                None,
                True,
                False,
                full_request_sent,
            )
        cached = first.get("cached") if isinstance(first.get("cached"), bool) else None
        raw_tier = first.get("cache_tier")
        cache_tier = (
            raw_tier.strip() if isinstance(raw_tier, str) and raw_tier.strip() else None
        )
        output_written = (
            first.get("output_written")
            if isinstance(first.get("output_written"), bool)
            else True
        )
        output_exists = not output_written
    if not bool(first.get("ok")):
        message = _backend_daemon_job_failure_message(first)
        if message is not None:
            return _BackendDaemonCompileResult(
                False,
                message,
                health,
                cached,
                cache_tier,
                output_written,
                False,
                full_request_sent,
            )
        return _BackendDaemonCompileResult(
            False,
            "backend daemon failed to compile job",
            health,
            cached,
            cache_tier,
            output_written,
            False,
            full_request_sent,
        )
    if output_written and not backend_output.exists():
        return _BackendDaemonCompileResult(
            False,
            "backend daemon reported success but output is missing",
            health,
            cached,
            cache_tier,
            output_written,
            False,
            full_request_sent,
        )
    output_exists = True
    return _BackendDaemonCompileResult(
        True,
        None,
        health,
        cached,
        cache_tier,
        output_written,
        output_exists,
        full_request_sent,
    )
