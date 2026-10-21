"""Consumer-neutral process and OS file-lock ownership authority."""

from __future__ import annotations

from collections.abc import Callable, Generator
import contextlib
from dataclasses import dataclass, field
from functools import lru_cache
import errno
import _thread
import os
import sys
from pathlib import Path
import threading
import time
from typing import BinaryIO, Protocol

from molt.default_paths import _default_molt_cache, shared_cache_lock_path


class FileLockTimeout(RuntimeError):
    """Another holder kept the lock for the whole timeout."""


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
    shared: bool = False
    lock: _HandleLock | None = None


# subprocess.Popen forks this process only on POSIX; Windows spawns. The fork
# audit reads this one fact, so a test can select either policy directly.
_POPEN_FORKS = os.name == "posix"

_IN_PROCESS_LOCK_REGISTRY: dict[str, _InProcessLockEntry] = {}
_IN_PROCESS_LOCK_REGISTRY_GUARD = threading.Lock()
_FILE_LOCK_LIFECYCLE_GUARD = threading.Lock()
_FILE_LOCK_LIFECYCLE_CONDITION = threading.Condition(_FILE_LOCK_LIFECYCLE_GUARD)
_LIVE_FILE_LOCK_HANDLES: dict[int, _FileLockHandle] = {}
# Handles between their release decision and the completed unlock. A
# concurrent release of the same handle returns only after it leaves here.
_RELEASING_FILE_LOCK_HANDLES: dict[int, _FileLockHandle] = {}
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
    if event == "subprocess.Popen" and not _POPEN_FORKS:
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
    global _RELEASING_FILE_LOCK_HANDLES
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
    _RELEASING_FILE_LOCK_HANDLES = {}
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
    return _file_lock_stream(os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o666))


def _open_existing_file_lock_handle(lock_path: Path) -> BinaryIO:
    """Open only a lock that exists; never create its file or directory.

    A custody record that another owner removed must stay removed, not
    reappear as a new lock.
    """
    return _file_lock_stream(os.open(lock_path, os.O_RDWR))


def _file_lock_stream(fd: int) -> BinaryIO:
    try:
        # Lock ownership is the OS region plus the in-process mutex, never file
        # contents. Windows permits locking byte zero beyond EOF. In particular,
        # opening a contender must not write a byte that another handle owns.
        return os.fdopen(fd, "r+b", buffering=0)
    except BaseException:
        os.close(fd)
        raise


class _MsvcrtLocking(Protocol):
    LK_NBLCK: int
    LK_UNLCK: int

    def locking(self, fd: int, mode: int, nbytes: int, /) -> None: ...


class _FcntlFlock(Protocol):
    LOCK_EX: int
    LOCK_SH: int
    LOCK_NB: int
    LOCK_UN: int

    def flock(self, fd: int, operation: int, /) -> None: ...


@dataclass(frozen=True, slots=True)
class _HandleLock:
    """A host's advisory lock on an open descriptor.

    `acquire_nonblocking` raises OSError when another holder has the lock.
    """

    acquire_nonblocking: Callable[[int], None]
    release: Callable[[int], None]


def _windows_handle_lock(msvcrt: _MsvcrtLocking) -> _HandleLock:
    # msvcrt locks a byte range; byte 0 stands for the whole file.
    return _HandleLock(
        acquire_nonblocking=lambda fd: msvcrt.locking(fd, msvcrt.LK_NBLCK, 1),
        release=lambda fd: msvcrt.locking(fd, msvcrt.LK_UNLCK, 1),
    )


def _posix_handle_lock(fcntl: _FcntlFlock, *, shared: bool = False) -> _HandleLock:
    return _HandleLock(
        acquire_nonblocking=lambda fd: fcntl.flock(
            fd, (fcntl.LOCK_SH if shared else fcntl.LOCK_EX) | fcntl.LOCK_NB
        ),
        release=lambda fd: fcntl.flock(fd, fcntl.LOCK_UN),
    )


def _windows_shared_handle_lock() -> _HandleLock:
    """A real Windows reader lock on the same byte as exclusive CRT locks."""
    if sys.platform != "win32":
        raise RuntimeError("Windows shared locking requires a Windows host")
    import ctypes
    from ctypes import wintypes
    import msvcrt

    class Overlapped(ctypes.Structure):
        _fields_ = [
            ("Internal", ctypes.c_size_t),
            ("InternalHigh", ctypes.c_size_t),
            ("Offset", wintypes.DWORD),
            ("OffsetHigh", wintypes.DWORD),
            ("hEvent", wintypes.HANDLE),
        ]

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.LockFileEx.argtypes = [
        wintypes.HANDLE,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.DWORD,
        ctypes.POINTER(Overlapped),
    ]
    kernel.LockFileEx.restype = wintypes.BOOL
    kernel.UnlockFileEx.argtypes = [
        wintypes.HANDLE,
        wintypes.DWORD,
        wintypes.DWORD,
        wintypes.DWORD,
        ctypes.POINTER(Overlapped),
    ]
    kernel.UnlockFileEx.restype = wintypes.BOOL

    def acquire(fd: int) -> None:
        overlap = Overlapped()
        # FAIL_IMMEDIATELY; no EXCLUSIVE_LOCK flag admits concurrent readers.
        if not kernel.LockFileEx(
            msvcrt.get_osfhandle(fd), 0x1, 0, 1, 0, ctypes.byref(overlap)
        ):
            raise ctypes.WinError(ctypes.get_last_error())

    def release(fd: int) -> None:
        overlap = Overlapped()
        if not kernel.UnlockFileEx(
            msvcrt.get_osfhandle(fd), 0, 1, 0, ctypes.byref(overlap)
        ):
            raise ctypes.WinError(ctypes.get_last_error())

    return _HandleLock(acquire_nonblocking=acquire, release=release)


# Selected once at import. A test drives another host's path by building its
# lock from a stand-in module, never by replacing os or msvcrt under code that
# other callers (the pytest phase record among them) share.
if os.name == "nt":
    import msvcrt

    _HOST_HANDLE_LOCK = _windows_handle_lock(msvcrt)
    _HOST_SHARED_HANDLE_LOCK_FACTORY = _windows_shared_handle_lock
else:
    import fcntl

    _HOST_HANDLE_LOCK = _posix_handle_lock(fcntl)

    def _HOST_SHARED_HANDLE_LOCK_FACTORY() -> _HandleLock:
        return _posix_handle_lock(fcntl, shared=True)


@lru_cache(maxsize=1)
def _shared_handle_lock() -> _HandleLock:
    return _HOST_SHARED_HANDLE_LOCK_FACTORY()


def _try_lock_file_handle(handle: BinaryIO) -> bool:
    return _try_lock_handle_with(handle, _HOST_HANDLE_LOCK)


def _try_lock_handle_with(handle: BinaryIO, lock: _HandleLock) -> bool:
    handle.seek(0)
    try:
        lock.acquire_nonblocking(handle.fileno())
    except OSError as exc:
        if exc.errno in (errno.EACCES, errno.EAGAIN):
            return False
        raise
    return True


def _unlock_file_handle(handle: BinaryIO, lock: _HandleLock | None = None) -> None:
    with contextlib.suppress(OSError):
        handle.seek(0)
        (lock or _HOST_HANDLE_LOCK).release(handle.fileno())


@contextlib.contextmanager
def _exclusive_record_file(
    path: Path,
    lock: _HandleLock,
    *,
    timeout_s: float,
    poll_s: float,
    timeout_message: str,
) -> Generator[BinaryIO, None, None]:
    """Hold `lock` on the record file at `path` and yield it for rewriting.

    For one writer per process, such as the pytest phase record. It uses only
    `lock`, `os` and `time`, never this module's other helpers, so it keeps
    working while a test replaces them. Readers in other processes take the
    same host lock through `_try_acquire_file_lock`.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o666)
    try:
        record = os.fdopen(fd, "r+b", buffering=0)
    except BaseException:
        os.close(fd)
        raise
    with record:
        deadline = time.monotonic() + timeout_s
        while True:
            # msvcrt locks from the current offset; byte 0 is the lock region.
            os.lseek(fd, 0, os.SEEK_SET)
            try:
                lock.acquire_nonblocking(fd)
                break
            except OSError as exc:
                if exc.errno not in (errno.EACCES, errno.EAGAIN):
                    raise
            if time.monotonic() >= deadline:
                raise FileLockTimeout(timeout_message)
            time.sleep(poll_s)
        try:
            opened, current = os.fstat(fd), os.stat(path)
            if (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino):
                raise RuntimeError(
                    f"record file was replaced while it was locked: {path}"
                )
            yield record
        finally:
            with contextlib.suppress(OSError):
                os.lseek(fd, 0, os.SEEK_SET)
                lock.release(fd)


def _try_acquire_file_lock(
    lock_path: Path, *, create: bool = True, shared: bool = False
) -> _FileLockHandle | None:
    with _file_lock_descriptor_action():
        return _try_acquire_file_lock_unserialized(
            lock_path, create=create, shared=shared
        )


def _try_acquire_file_lock_unserialized(
    lock_path: Path, *, create: bool = True, shared: bool = False
) -> _FileLockHandle | None:
    registry_key, entry = _in_process_lock_reserve(lock_path)
    if not shared and not entry.mutex.acquire(blocking=False):
        _in_process_lock_drop(registry_key, entry)
        return None
    try:
        opener = _open_file_lock_handle if create else _open_existing_file_lock_handle
        file_handle = opener(lock_path)
    except BaseException:
        if not shared:
            entry.mutex.release()
        _in_process_lock_drop(registry_key, entry)
        raise
    handle = None
    try:
        lock = _shared_handle_lock() if shared else None
        locked = (
            _try_lock_handle_with(file_handle, lock)
            if lock is not None
            else _try_lock_file_handle(file_handle)
        )
        if not locked:
            file_handle.close()
            if not shared:
                entry.mutex.release()
            _in_process_lock_drop(registry_key, entry)
            return None
        handle = _FileLockHandle(
            file=file_handle,
            registry_key=registry_key,
            entry=entry,
            shared=shared,
            lock=lock,
        )
        with _FILE_LOCK_LIFECYCLE_CONDITION:
            _LIVE_FILE_LOCK_HANDLES[id(handle)] = handle
        # Shared handles rely on the actual per-open OS reader lock, including
        # in one process; they never acquire or transfer the exclusive mutex.
        return handle
    except BaseException:
        if handle is not None:
            with _FILE_LOCK_LIFECYCLE_CONDITION:
                _LIVE_FILE_LOCK_HANDLES.pop(id(handle), None)
                handle.released = True
        file_handle.close()
        if not shared:
            entry.mutex.release()
        _in_process_lock_drop(registry_key, entry)
        raise


def _acquire_file_lock(
    lock_path: Path,
    *,
    timeout_s: float | None,
    timeout_message: str,
    poll_s: float = 0.05,
    shared: bool = False,
) -> _FileLockHandle:
    deadline = time.monotonic() + timeout_s if timeout_s is not None else None
    while True:
        handle = (
            _try_acquire_file_lock(lock_path, shared=True)
            if shared
            else _try_acquire_file_lock(lock_path)
        )
        if handle is not None:
            return handle
        if deadline is not None and time.monotonic() >= deadline:
            raise FileLockTimeout(timeout_message)
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
def _file_lock_owned_operation(
    handle: _FileLockHandle, *, expected_lock_path: Path, shared: bool = False
):
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
            or handle.shared != shared
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


def _await_concurrent_release(handle: _FileLockHandle) -> None:
    """Return once another thread's release of ``handle`` has freed the lock.

    Call with the lifecycle condition held. A copy of a handle is never the
    one in flight, so it does not wait.
    """
    _FILE_LOCK_LIFECYCLE_CONDITION.wait_for(
        lambda: _RELEASING_FILE_LOCK_HANDLES.get(id(handle)) is not handle
    )


def _release_file_lock(handle: _FileLockHandle) -> None:
    """Release ``handle``; on return the lock is free, whichever thread freed it."""
    _require_file_lock_operation_admission()
    if handle.owner_process_id != os.getpid():
        return
    # Do not enter a draining descriptor action while waiting for a pin. The
    # owner may perform nested descriptor work to complete that pin.
    with _FILE_LOCK_LIFECYCLE_CONDITION:
        if handle.released or _LIVE_FILE_LOCK_HANDLES.get(id(handle)) is not handle:
            _await_concurrent_release(handle)
            return
        if threading.get_ident() in handle.operation_owners:
            raise RuntimeError("cannot release file lock during its owned operation")
        handle.release_requested = True
        _FILE_LOCK_LIFECYCLE_CONDITION.notify_all()
        while handle.operation_owners:
            _FILE_LOCK_LIFECYCLE_CONDITION.wait()
            if handle.released or _LIVE_FILE_LOCK_HANDLES.get(id(handle)) is not handle:
                _await_concurrent_release(handle)
                return
    with _file_lock_descriptor_action():
        with _FILE_LOCK_LIFECYCLE_CONDITION:
            if handle.released or _LIVE_FILE_LOCK_HANDLES.get(id(handle)) is not handle:
                _await_concurrent_release(handle)
                return
            handle.released = True
            del _LIVE_FILE_LOCK_HANDLES[id(handle)]
            _RELEASING_FILE_LOCK_HANDLES[id(handle)] = handle
        try:
            if not handle.file.closed:
                if handle.shared:
                    _unlock_file_handle(handle.file, handle.lock)
                else:
                    _unlock_file_handle(handle.file)
        finally:
            try:
                handle.file.close()
            finally:
                try:
                    if not handle.shared:
                        handle.entry.mutex.release()
                    _in_process_lock_drop(handle.registry_key, handle.entry)
                finally:
                    with _FILE_LOCK_LIFECYCLE_CONDITION:
                        del _RELEASING_FILE_LOCK_HANDLES[id(handle)]
                        _FILE_LOCK_LIFECYCLE_CONDITION.notify_all()


def _parse_lock_timeout(raw: str, *, default_s: float | None) -> float | None:
    raw = raw.strip()
    if not raw:
        return default_s
    try:
        parsed = float(raw)
    except ValueError:
        return default_s
    return parsed if parsed > 0 else None


@contextlib.contextmanager
def _shared_cache_lock(name: str, *, cache_root: Path | None = None):
    if cache_root is None:
        cache_root = _default_molt_cache()
    lock_path = shared_cache_lock_path(cache_root, name)
    timeout_raw = os.environ.get("MOLT_BUILD_LOCK_TIMEOUT", "").strip()
    lock_timeout = _parse_lock_timeout(timeout_raw, default_s=300.0)
    timeout_label = "unbounded" if lock_timeout is None else f"{lock_timeout:.1f}s"
    handle = _acquire_file_lock(
        lock_path,
        timeout_s=lock_timeout,
        timeout_message=(
            "Timed out waiting for shared cache lock "
            f"{lock_path} after {timeout_label}. "
            "Check for stale molt build/backend helper processes."
        ),
    )
    try:
        yield
    finally:
        _release_file_lock(handle)
