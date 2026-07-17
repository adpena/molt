from __future__ import annotations

import contextlib
from contextlib import contextmanager
from dataclasses import dataclass
import errno
import functools
import _thread
import os
from pathlib import Path
import threading
import time
from typing import BinaryIO

from molt.cli.runtime_paths import _build_state_root


@dataclass
class _InProcessLockEntry:
    mutex: _thread.LockType
    users: int = 0


@dataclass
class _FileLockHandle:
    file: BinaryIO
    registry_key: str
    entry: _InProcessLockEntry


_IN_PROCESS_LOCK_REGISTRY: dict[str, _InProcessLockEntry] = {}
_IN_PROCESS_LOCK_REGISTRY_GUARD = threading.Lock()


def _in_process_lock_key(lock_path: Path) -> str:
    return os.path.normcase(os.fspath(lock_path.resolve(strict=False)))


def _reset_in_process_lock_registry_after_fork() -> None:
    global _IN_PROCESS_LOCK_REGISTRY, _IN_PROCESS_LOCK_REGISTRY_GUARD
    # A forked child owns no parent threads. Replacing both objects avoids an
    # inherited mutex/registry guard that was locked by a vanished thread.
    _IN_PROCESS_LOCK_REGISTRY = {}
    _IN_PROCESS_LOCK_REGISTRY_GUARD = threading.Lock()


if hasattr(os, "register_at_fork"):
    os.register_at_fork(after_in_child=_reset_in_process_lock_registry_after_fork)


def _in_process_lock_reserve(lock_path: Path) -> tuple[str, _InProcessLockEntry]:
    key = _in_process_lock_key(lock_path)
    with _IN_PROCESS_LOCK_REGISTRY_GUARD:
        entry = _IN_PROCESS_LOCK_REGISTRY.get(key)
        if entry is None:
            entry = _InProcessLockEntry(mutex=threading.Lock())
            _IN_PROCESS_LOCK_REGISTRY[key] = entry
        entry.users += 1
    return key, entry


def _in_process_lock_drop(key: str, entry: _InProcessLockEntry) -> None:
    with _IN_PROCESS_LOCK_REGISTRY_GUARD:
        entry.users -= 1
        if entry.users == 0 and _IN_PROCESS_LOCK_REGISTRY.get(key) is entry:
            del _IN_PROCESS_LOCK_REGISTRY[key]


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


def _try_acquire_file_lock(lock_path: Path) -> _FileLockHandle | None:
    registry_key, entry = _in_process_lock_reserve(lock_path)
    if not entry.mutex.acquire(blocking=False):
        _in_process_lock_drop(registry_key, entry)
        return None
    try:
        file_handle = _open_file_lock_handle(lock_path)
    except BaseException:
        entry.mutex.release()
        _in_process_lock_drop(registry_key, entry)
        raise
    try:
        if not _try_lock_file_handle(file_handle):
            file_handle.close()
            entry.mutex.release()
            _in_process_lock_drop(registry_key, entry)
            return None
        _write_lock_holder_pid(file_handle)
        return _FileLockHandle(
            file=file_handle,
            registry_key=registry_key,
            entry=entry,
        )
    except BaseException:
        file_handle.close()
        entry.mutex.release()
        _in_process_lock_drop(registry_key, entry)
        raise


def _acquire_file_lock(
    lock_path: Path,
    *,
    timeout_s: float | None,
    timeout_message: str,
    poll_s: float = 0.05,
) -> _FileLockHandle:
    deadline = time.monotonic() + timeout_s if timeout_s is not None else None
    while True:
        handle = _try_acquire_file_lock(lock_path)
        if handle is not None:
            return handle
        if deadline is not None and time.monotonic() >= deadline:
            raise RuntimeError(timeout_message)
        time.sleep(poll_s)


def _release_file_lock(handle: _FileLockHandle) -> None:
    try:
        _unlock_file_handle(handle.file)
    finally:
        try:
            handle.file.close()
        finally:
            handle.entry.mutex.release()
            _in_process_lock_drop(handle.registry_key, handle.entry)


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
