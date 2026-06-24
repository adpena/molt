from __future__ import annotations

import contextlib
from contextlib import contextmanager
import errno
import functools
import os
from pathlib import Path
import time
from typing import BinaryIO

from molt.cli.runtime_paths import _build_state_root


@functools.lru_cache(maxsize=256)
def _build_lock_dir_cached(project_root_str: str, build_state_root_str: str) -> Path:
    return Path(build_state_root_str) / "build_locks"


def _open_file_lock_handle(lock_path: Path) -> BinaryIO:
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o666)
    handle = os.fdopen(fd, "r+b", buffering=0)
    try:
        if os.fstat(fd).st_size == 0:
            handle.write(b"\0")
            handle.flush()
        handle.seek(0)
    except BaseException:
        handle.close()
        raise
    return handle


def _try_lock_file_handle(handle: BinaryIO) -> bool:
    handle.seek(0)
    if os.name == "nt":
        import msvcrt

        try:
            msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
        except OSError:
            return False
        return True

    import fcntl

    try:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError as exc:
        if exc.errno in (errno.EACCES, errno.EAGAIN):
            return False
        raise
    return True


def _unlock_file_handle(handle: BinaryIO) -> None:
    with contextlib.suppress(OSError, ImportError):
        handle.seek(0)
        if os.name == "nt":
            import msvcrt

            msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
        else:
            import fcntl

            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def _write_lock_holder_pid(handle: BinaryIO) -> None:
    with contextlib.suppress(OSError):
        handle.seek(0)
        handle.truncate(0)
        handle.write(f"{os.getpid()}\n".encode("ascii"))
        handle.flush()
        handle.seek(0)


def _try_acquire_file_lock(lock_path: Path) -> BinaryIO | None:
    handle = _open_file_lock_handle(lock_path)
    try:
        if not _try_lock_file_handle(handle):
            handle.close()
            return None
        _write_lock_holder_pid(handle)
        return handle
    except BaseException:
        handle.close()
        raise


def _acquire_file_lock(
    lock_path: Path,
    *,
    timeout_s: float | None,
    timeout_message: str,
    poll_s: float = 0.05,
) -> BinaryIO:
    deadline = time.monotonic() + timeout_s if timeout_s is not None else None
    while True:
        handle = _try_acquire_file_lock(lock_path)
        if handle is not None:
            return handle
        if deadline is not None and time.monotonic() >= deadline:
            raise RuntimeError(timeout_message)
        time.sleep(poll_s)


def _release_file_lock(handle: BinaryIO) -> None:
    try:
        _unlock_file_handle(handle)
    finally:
        handle.close()


def _parse_lock_timeout(raw: str, *, default_s: float | None) -> float | None:
    raw = raw.strip()
    if not raw:
        return default_s
    try:
        parsed = float(raw)
    except ValueError:
        return default_s
    return parsed if parsed > 0 else None


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
