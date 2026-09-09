"""Build-state path and timeout policy over the shared file-lock authority."""

from __future__ import annotations

from contextlib import contextmanager
import functools
import os
from pathlib import Path

from molt.file_locks import _acquire_file_lock, _release_file_lock, _parse_lock_timeout
from molt.cli.runtime_paths import _build_state_root


@functools.lru_cache(maxsize=256)
def _build_lock_dir_cached(project_root_str: str, build_state_root_str: str) -> Path:
    return Path(build_state_root_str) / "build_locks"


@contextmanager
def _build_lock(project_root: Path, name: str):
    lock_dir = _build_lock_dir_cached(
        os.fspath(project_root),
        os.fspath(_build_state_root(project_root)),
    )
    # The build-state root already carries target/session isolation. When an
    # operator explicitly shares a target/build-state root, mutable Cargo
    # artifacts must share the same lock regardless of MOLT_SESSION_ID.
    lock_path = lock_dir / f"{name}.lock"
    lock_timeout = _parse_lock_timeout(
        os.environ.get("MOLT_BUILD_LOCK_TIMEOUT", ""),
        default_s=300.0,
    )
    timeout_label = "unbounded" if lock_timeout is None else f"{lock_timeout:.1f}s"
    handle = _acquire_file_lock(
        lock_path,
        timeout_s=lock_timeout,
        timeout_message=(
            f"Timed out waiting for build lock {lock_path} after {timeout_label}. "
            "Check for stale molt build/backend helper processes."
        ),
    )
    try:
        yield
    finally:
        _release_file_lock(handle)
