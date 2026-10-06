"""Consumer-neutral process and OS file-lock ownership authority."""

from __future__ import annotations

import contextlib
from dataclasses import dataclass, field
import errno
import _thread
import os
import sys
from pathlib import Path
import threading
import time
from typing import BinaryIO


@dataclass
class _InProcessLockEntry:
    mutex: _thread.LockType
    users: int = 0


@dataclass
class _FileLockHandle:
    file: BinaryIO
    registry_key: str
    entry: _InProcessLockEntry
    owner_process_id: int = field(default_factory=os.getpid)
    released: bool = False
    operation_owners: dict[int, int] = field(default_factory=dict, repr=False)
    release_requested: bool = False


_IN_PROCESS_LOCK_REGISTRY: dict[str, _InProcessLockEntry] = {}
_IN_PROCESS_LOCK_REGISTRY_GUARD = threading.Lock()
_FILE_LOCK_LIFECYCLE_GUARD = threading.Lock()
_FILE_LOCK_LIFECYCLE_CONDITION = threading.Condition(_FILE_LOCK_LIFECYCLE_GUARD)
_LIVE_FILE_LOCK_HANDLES: dict[int, _FileLockHandle] = {}
_FILE_LOCK_DESCRIPTOR_ACTIONS: dict[int, int] = {}
_FILE_LOCK_ATOMIC_LOCAL = threading.local()
_FILE_LOCK_AUDIT_INSTALL_TOKEN = object()
_FILE_LOCK_AUDIT_INSTALL_VERIFIED = False
_FILE_LOCK_FORK_CLEANUP_ERROR: str | None = None


def _in_process_lock_key(lock_path: Path) -> str:
    return os.path.normcase(os.fspath(lock_path.resolve(strict=False)))


def _require_file_lock_operation_admission() -> None:
    if _FILE_LOCK_FORK_CLEANUP_ERROR is not None:
        raise RuntimeError(
            "inherited file-lock cleanup failed; child custody is unavailable: "
            + _FILE_LOCK_FORK_CLEANUP_ERROR
        )
    if getattr(_FILE_LOCK_ATOMIC_LOCAL, "fork_protocol_depth", 0):
        raise RuntimeError(
            "file-lock operation must be deferred until fork lifecycle callbacks end"
        )


@contextlib.contextmanager
def _file_lock_atomic_mutation(reason: str):
    """Reject same-thread Python fork before callbacks/syscall during mutation.

    This is an internal custody invariant, not a spawn policy. Ordinary fork
    while a quiescent lock is held remains allowed. Raw libc/ctypes fork is not
    a Python-owned audited API and is not covered by this authority.
    """
    _require_file_lock_operation_admission()
    prior = getattr(_FILE_LOCK_ATOMIC_LOCAL, "reasons", ())
    _FILE_LOCK_ATOMIC_LOCAL.reasons = (*prior, reason)
    try:
        yield
    finally:
        _FILE_LOCK_ATOMIC_LOCAL.reasons = prior


def _file_lock_fork_audit(event: str, _args: tuple[object, ...]) -> None:
    global _FILE_LOCK_AUDIT_INSTALL_VERIFIED
    if event == "molt.file_lock.fork_admission":
        if len(_args) == 1 and _args[0] is _FILE_LOCK_AUDIT_INSTALL_TOKEN:
            _FILE_LOCK_AUDIT_INSTALL_VERIFIED = True
        return
    if event == "subprocess.Popen" and os.name != "posix":
        return
    if event not in ("os.fork", "os.forkpty", "subprocess.Popen"):
        return
    reasons = getattr(_FILE_LOCK_ATOMIC_LOCAL, "reasons", ())
    if reasons or getattr(_FILE_LOCK_ATOMIC_LOCAL, "fork_protocol_depth", 0):
        raise RuntimeError(
            f"{event} must be deferred until the current atomic custody operation ends"
            + (f": {reasons[-1]}" if reasons else ": fork lifecycle callback")
        )


def _enter_file_lock_fork_protocol() -> None:
    _FILE_LOCK_ATOMIC_LOCAL.fork_protocol_depth = (
        getattr(_FILE_LOCK_ATOMIC_LOCAL, "fork_protocol_depth", 0) + 1
    )


def _leave_file_lock_fork_protocol() -> None:
    _FILE_LOCK_ATOMIC_LOCAL.fork_protocol_depth -= 1


@contextlib.contextmanager
def _file_lock_descriptor_action():
    """Drain descriptor transitions before fork, with no mutex across I/O."""
    thread_id = threading.get_ident()
    with _file_lock_atomic_mutation("file-lock descriptor birth/close"):
        with _FILE_LOCK_LIFECYCLE_CONDITION:
            _FILE_LOCK_DESCRIPTOR_ACTIONS[thread_id] = (
                _FILE_LOCK_DESCRIPTOR_ACTIONS.get(thread_id, 0) + 1
            )
        try:
            yield
        finally:
            with _FILE_LOCK_LIFECYCLE_CONDITION:
                count = _FILE_LOCK_DESCRIPTOR_ACTIONS[thread_id] - 1
                if count:
                    _FILE_LOCK_DESCRIPTOR_ACTIONS[thread_id] = count
                else:
                    del _FILE_LOCK_DESCRIPTOR_ACTIONS[thread_id]
                _FILE_LOCK_LIFECYCLE_CONDITION.notify_all()


def _before_file_lock_fork() -> None:
    # CPython's audit event precedes these callbacks, so a same-thread fork
    # cannot enter while its own descriptor transition is outstanding.
    if getattr(_FILE_LOCK_ATOMIC_LOCAL, "reasons", ()):
        raise RuntimeError("fork lifecycle entered during atomic custody mutation")
    _FILE_LOCK_ATOMIC_LOCAL.fork_gate_held = False
    _enter_file_lock_fork_protocol()
    _FILE_LOCK_ATOMIC_LOCAL.fork_protocol_entered = True
    _FILE_LOCK_LIFECYCLE_CONDITION.acquire()
    _FILE_LOCK_ATOMIC_LOCAL.fork_gate_held = True
    while _FILE_LOCK_DESCRIPTOR_ACTIONS:
        _FILE_LOCK_LIFECYCLE_CONDITION.wait()
    # Keep the gate only over the fork itself. All birth/close I/O has drained.


def _after_file_lock_fork_parent() -> None:
    try:
        if getattr(_FILE_LOCK_ATOMIC_LOCAL, "fork_gate_held", False):
            _FILE_LOCK_ATOMIC_LOCAL.fork_gate_held = False
            _FILE_LOCK_LIFECYCLE_CONDITION.release()
    finally:
        if getattr(_FILE_LOCK_ATOMIC_LOCAL, "fork_protocol_entered", False):
            _FILE_LOCK_ATOMIC_LOCAL.fork_protocol_entered = False
            _leave_file_lock_fork_protocol()


def _reset_in_process_lock_registry_after_fork() -> None:
    global _IN_PROCESS_LOCK_REGISTRY, _IN_PROCESS_LOCK_REGISTRY_GUARD
    global _FILE_LOCK_LIFECYCLE_GUARD, _LIVE_FILE_LOCK_HANDLES
    global _FILE_LOCK_LIFECYCLE_CONDITION, _FILE_LOCK_DESCRIPTOR_ACTIONS
    global _FILE_LOCK_ATOMIC_LOCAL, _FILE_LOCK_FORK_CLEANUP_ERROR
    # Every descriptor transition drained before fork. Close inherited stream
    # copies without LOCK_UN, which would revoke the parent's flock ownership.
    cleanup_errors: list[str] = (
        [_FILE_LOCK_FORK_CLEANUP_ERROR]
        if _FILE_LOCK_FORK_CLEANUP_ERROR is not None
        else []
    )
    for handle in _LIVE_FILE_LOCK_HANDLES.values():
        handle.released = True
        try:
            handle.file.close()
        except BaseException as exc:
            # Profile/close callbacks may fail before the descriptor closes.
            # Still invalidate every inherited capability and reset all guards,
            # but do not admit new child custody with an unclosed descriptor.
            cleanup_errors.append(type(exc).__name__)
        if not handle.file.closed:
            cleanup_errors.append("inherited stream remains open")
    _LIVE_FILE_LOCK_HANDLES = {}
    _IN_PROCESS_LOCK_REGISTRY = {}
    _IN_PROCESS_LOCK_REGISTRY_GUARD = threading.Lock()
    _FILE_LOCK_LIFECYCLE_GUARD = threading.Lock()
    _FILE_LOCK_LIFECYCLE_CONDITION = threading.Condition(_FILE_LOCK_LIFECYCLE_GUARD)
    _FILE_LOCK_DESCRIPTOR_ACTIONS = {}
    _FILE_LOCK_ATOMIC_LOCAL = threading.local()
    _FILE_LOCK_FORK_CLEANUP_ERROR = (
        "; ".join(cleanup_errors) if cleanup_errors else None
    )


# Unlike at-fork callback exceptions, audit exceptions abort os.fork/forkpty
# before PyOS_BeforeFork and the syscall in CPython 3.12, 3.13 and 3.14.
sys.addaudithook(_file_lock_fork_audit)
# Existing hooks may silently reject registration by raising RuntimeError.
# Prove our installed hook observed its private challenge before admitting locks.
sys.audit("molt.file_lock.fork_admission", _FILE_LOCK_AUDIT_INSTALL_TOKEN)
if not _FILE_LOCK_AUDIT_INSTALL_VERIFIED:
    raise RuntimeError(
        "file-lock pre-fork admission audit hook registration was blocked"
    )
if hasattr(os, "register_at_fork"):
    os.register_at_fork(
        before=_before_file_lock_fork,
        after_in_parent=_after_file_lock_fork_parent,
        after_in_child=_reset_in_process_lock_registry_after_fork,
    )


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


def _open_file_lock_handle(lock_path: Path) -> BinaryIO:
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o666)
    try:
        # Lock ownership is the OS region plus the in-process mutex, never file
        # contents. Windows permits locking byte zero beyond EOF. In particular,
        # opening a contender must not write a byte that another handle owns.
        return os.fdopen(fd, "r+b", buffering=0)
    except BaseException:
        os.close(fd)
        raise


def _try_lock_file_handle(handle: BinaryIO) -> bool:
    handle.seek(0)
    try:
        if os.name == "nt":
            import msvcrt

            msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl

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


def _try_acquire_file_lock(lock_path: Path) -> _FileLockHandle | None:
    with _file_lock_descriptor_action():
        return _try_acquire_file_lock_unserialized(lock_path)


def _try_acquire_file_lock_unserialized(lock_path: Path) -> _FileLockHandle | None:
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
        handle = _FileLockHandle(
            file=file_handle,
            registry_key=registry_key,
            entry=entry,
        )
        with _FILE_LOCK_LIFECYCLE_CONDITION:
            _LIVE_FILE_LOCK_HANDLES[id(handle)] = handle
        return handle
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


def _file_lock_is_owned(handle: _FileLockHandle) -> bool:
    _require_file_lock_operation_admission()
    if handle.owner_process_id != os.getpid():
        return False
    with _FILE_LOCK_LIFECYCLE_GUARD:
        return (
            not handle.released
            and not handle.file.closed
            and _LIVE_FILE_LOCK_HANDLES.get(id(handle)) is handle
        )


@contextlib.contextmanager
def _file_lock_owned_operation(handle: _FileLockHandle, *, expected_lock_path: Path):
    """Pin exact custody across I/O without holding the global lifecycle mutex.

    Revocation waits for this reservation; a callback on its owning thread
    cannot revoke its own capability or wait for itself.
    """
    _require_file_lock_operation_admission()
    expected_key = _in_process_lock_key(expected_lock_path)
    thread_id = threading.get_ident()
    with _FILE_LOCK_LIFECYCLE_CONDITION:
        if (
            handle.owner_process_id != os.getpid()
            or handle.released
            or handle.file.closed
            or (handle.release_requested and thread_id not in handle.operation_owners)
            or _LIVE_FILE_LOCK_HANDLES.get(id(handle)) is not handle
            or handle.registry_key != expected_key
        ):
            raise ValueError("file-lock operation lacks exact live ownership")
        handle.operation_owners[thread_id] = (
            handle.operation_owners.get(thread_id, 0) + 1
        )
    try:
        with _file_lock_atomic_mutation("pinned file-lock publication/reclamation"):
            yield
    finally:
        with _FILE_LOCK_LIFECYCLE_CONDITION:
            count = handle.operation_owners[thread_id] - 1
            if count:
                handle.operation_owners[thread_id] = count
            else:
                del handle.operation_owners[thread_id]
            _FILE_LOCK_LIFECYCLE_CONDITION.notify_all()


def _release_file_lock(handle: _FileLockHandle) -> None:
    _require_file_lock_operation_admission()
    if handle.owner_process_id != os.getpid():
        return
    # Do not enter a draining descriptor action while waiting for a pin. The
    # owner may perform nested descriptor work to complete that pin.
    with _FILE_LOCK_LIFECYCLE_CONDITION:
        if handle.released or _LIVE_FILE_LOCK_HANDLES.get(id(handle)) is not handle:
            return
        if threading.get_ident() in handle.operation_owners:
            raise RuntimeError("cannot release file lock during its owned operation")
        handle.release_requested = True
        _FILE_LOCK_LIFECYCLE_CONDITION.notify_all()
        while handle.operation_owners:
            _FILE_LOCK_LIFECYCLE_CONDITION.wait()
            if handle.released or _LIVE_FILE_LOCK_HANDLES.get(id(handle)) is not handle:
                return
    with _file_lock_descriptor_action():
        with _FILE_LOCK_LIFECYCLE_CONDITION:
            if handle.released or _LIVE_FILE_LOCK_HANDLES.get(id(handle)) is not handle:
                return
            handle.released = True
            del _LIVE_FILE_LOCK_HANDLES[id(handle)]
        try:
            if not handle.file.closed:
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
