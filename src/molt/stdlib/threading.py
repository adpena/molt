"""Capability-gated threading for Molt."""

# Threading behavior in this module is runtime-intrinsic-backed; parity gaps are
# tracked in docs/spec/STATUS.md and the stdlib compatibility matrix.

from __future__ import annotations

import sys
from _intrinsics import require_intrinsic as _require_intrinsic

Any = object  # type: ignore[assignment]
Callable = object  # type: ignore[assignment]


_MOLT_THREAD_JOIN = _require_intrinsic("molt_thread_join")
_MOLT_THREAD_IS_ALIVE = _require_intrinsic("molt_thread_is_alive")
_MOLT_THREAD_IDENT = _require_intrinsic("molt_thread_ident")
_MOLT_THREAD_NATIVE_ID = _require_intrinsic("molt_thread_native_id")
_MOLT_THREAD_CURRENT_IDENT = _require_intrinsic("molt_thread_current_ident")
_MOLT_THREAD_CURRENT_NATIVE_ID = _require_intrinsic(
    "molt_thread_current_native_id")
_MOLT_THREAD_SPAWN_SHARED = _require_intrinsic("molt_thread_spawn_shared")
_MOLT_THREAD_DROP = _require_intrinsic("molt_thread_drop")
_MOLT_THREAD_STACK_SIZE_GET = _require_intrinsic(
    "molt_thread_stack_size_get")
_MOLT_THREAD_STACK_SIZE_SET = _require_intrinsic(
    "molt_thread_stack_size_set")
_MOLT_THREAD_REGISTRY_SET_MAIN = _require_intrinsic(
    "molt_thread_registry_set_main")
_MOLT_THREAD_REGISTRY_REGISTER = _require_intrinsic(
    "molt_thread_registry_register")
_MOLT_THREAD_REGISTRY_FORGET = _require_intrinsic(
    "molt_thread_registry_forget")
_MOLT_THREAD_REGISTRY_SNAPSHOT = _require_intrinsic(
    "molt_thread_registry_snapshot")
_MOLT_THREAD_REGISTRY_CURRENT = _require_intrinsic(
    "molt_thread_registry_current")
_MOLT_THREAD_REGISTRY_ACTIVE_COUNT = _require_intrinsic(
    "molt_thread_registry_active_count")
_MOLT_LOCK_NEW = _require_intrinsic("molt_lock_new")
_MOLT_LOCK_ACQUIRE = _require_intrinsic("molt_lock_acquire")
_MOLT_LOCK_RELEASE = _require_intrinsic("molt_lock_release")
_MOLT_LOCK_LOCKED = _require_intrinsic("molt_lock_locked")
_MOLT_LOCK_DROP = _require_intrinsic("molt_lock_drop")
_MOLT_RLOCK_NEW = _require_intrinsic("molt_rlock_new")
_MOLT_RLOCK_ACQUIRE = _require_intrinsic("molt_rlock_acquire")
_MOLT_RLOCK_RELEASE = _require_intrinsic("molt_rlock_release")
_MOLT_RLOCK_LOCKED = _require_intrinsic("molt_rlock_locked")
_MOLT_RLOCK_IS_OWNED = _require_intrinsic("molt_rlock_is_owned")
_MOLT_RLOCK_RELEASE_SAVE = _require_intrinsic("molt_rlock_release_save")
_MOLT_RLOCK_ACQUIRE_RESTORE = _require_intrinsic(
    "molt_rlock_acquire_restore")
_MOLT_RLOCK_DROP = _require_intrinsic("molt_rlock_drop")
_MOLT_CONDITION_NEW = _require_intrinsic("molt_condition_new")
_MOLT_CONDITION_WAIT = _require_intrinsic("molt_condition_wait")
_MOLT_CONDITION_WAIT_FOR = _require_intrinsic("molt_condition_wait_for")
_MOLT_CONDITION_NOTIFY = _require_intrinsic("molt_condition_notify")
_MOLT_CONDITION_DROP = _require_intrinsic("molt_condition_drop")
_MOLT_EVENT_NEW = _require_intrinsic("molt_event_new")
_MOLT_EVENT_SET = _require_intrinsic("molt_event_set")
_MOLT_EVENT_CLEAR = _require_intrinsic("molt_event_clear")
_MOLT_EVENT_IS_SET = _require_intrinsic("molt_event_is_set")
_MOLT_EVENT_WAIT = _require_intrinsic("molt_event_wait")
_MOLT_EVENT_DROP = _require_intrinsic("molt_event_drop")
_MOLT_SEMAPHORE_NEW = _require_intrinsic("molt_semaphore_new")
_MOLT_SEMAPHORE_ACQUIRE = _require_intrinsic("molt_semaphore_acquire")
_MOLT_SEMAPHORE_RELEASE = _require_intrinsic("molt_semaphore_release")
_MOLT_SEMAPHORE_DROP = _require_intrinsic("molt_semaphore_drop")
_MOLT_BARRIER_NEW = _require_intrinsic("molt_barrier_new")
_MOLT_BARRIER_WAIT = _require_intrinsic("molt_barrier_wait")
_MOLT_BARRIER_ABORT = _require_intrinsic("molt_barrier_abort")
_MOLT_BARRIER_RESET = _require_intrinsic("molt_barrier_reset")
_MOLT_BARRIER_PARTIES = _require_intrinsic("molt_barrier_parties")
_MOLT_BARRIER_N_WAITING = _require_intrinsic("molt_barrier_n_waiting")
_MOLT_BARRIER_BROKEN = _require_intrinsic("molt_barrier_broken")
_MOLT_BARRIER_DROP = _require_intrinsic("molt_barrier_drop")
_MOLT_LOCAL_NEW = _require_intrinsic("molt_local_new")
_MOLT_LOCAL_GET_DICT = _require_intrinsic("molt_local_get_dict")
_MOLT_LOCAL_DROP = _require_intrinsic("molt_local_drop")
_MOLT_MODULE_CACHE_SET = _require_intrinsic("molt_module_cache_set")


def _expect_int(value: object) -> int:
    return int(value)


# Use a list + loop instead of a 55-element tuple literal to reduce IR size.
_ALL_INTRINSICS = [
    _MOLT_THREAD_JOIN, _MOLT_THREAD_IS_ALIVE, _MOLT_THREAD_IDENT,
    _MOLT_THREAD_NATIVE_ID, _MOLT_THREAD_CURRENT_IDENT,
    _MOLT_THREAD_CURRENT_NATIVE_ID, _MOLT_THREAD_SPAWN_SHARED,
    _MOLT_THREAD_DROP, _MOLT_THREAD_STACK_SIZE_GET,
    _MOLT_THREAD_STACK_SIZE_SET, _MOLT_THREAD_REGISTRY_SET_MAIN,
    _MOLT_THREAD_REGISTRY_REGISTER, _MOLT_THREAD_REGISTRY_FORGET,
    _MOLT_THREAD_REGISTRY_SNAPSHOT, _MOLT_THREAD_REGISTRY_CURRENT,
    _MOLT_THREAD_REGISTRY_ACTIVE_COUNT,
    _MOLT_LOCK_NEW, _MOLT_LOCK_ACQUIRE, _MOLT_LOCK_RELEASE,
    _MOLT_LOCK_LOCKED, _MOLT_LOCK_DROP,
    _MOLT_RLOCK_NEW, _MOLT_RLOCK_ACQUIRE, _MOLT_RLOCK_RELEASE,
    _MOLT_RLOCK_LOCKED, _MOLT_RLOCK_IS_OWNED,
    _MOLT_RLOCK_RELEASE_SAVE, _MOLT_RLOCK_ACQUIRE_RESTORE, _MOLT_RLOCK_DROP,
    _MOLT_CONDITION_NEW, _MOLT_CONDITION_WAIT, _MOLT_CONDITION_WAIT_FOR,
    _MOLT_CONDITION_NOTIFY, _MOLT_CONDITION_DROP,
    _MOLT_EVENT_NEW, _MOLT_EVENT_SET, _MOLT_EVENT_CLEAR,
    _MOLT_EVENT_IS_SET, _MOLT_EVENT_WAIT, _MOLT_EVENT_DROP,
    _MOLT_SEMAPHORE_NEW, _MOLT_SEMAPHORE_ACQUIRE, _MOLT_SEMAPHORE_RELEASE,
    _MOLT_SEMAPHORE_DROP,
    _MOLT_BARRIER_NEW, _MOLT_BARRIER_WAIT, _MOLT_BARRIER_ABORT,
    _MOLT_BARRIER_RESET, _MOLT_BARRIER_PARTIES, _MOLT_BARRIER_N_WAITING,
    _MOLT_BARRIER_BROKEN, _MOLT_BARRIER_DROP,
    _MOLT_LOCAL_NEW, _MOLT_LOCAL_GET_DICT, _MOLT_LOCAL_DROP,
]

_HAVE_INTRINSICS = True
for _fn in _ALL_INTRINSICS:
    if not callable(_fn):
        _HAVE_INTRINSICS = False
        break


def _register_module_cache() -> None:
    module = sys.modules.get(__name__)
    if module is None:
        return
    _MOLT_MODULE_CACHE_SET(__name__, module)
    if __name__ != "threading":
        _MOLT_MODULE_CACHE_SET("threading", module)


if not _HAVE_INTRINSICS:
    raise RuntimeError("threading intrinsics unavailable")
else:
    _register_module_cache()

    __all__ = [
        "Thread",
        "Lock",
        "RLock",
        "Condition",
        "Event",
        "Semaphore",
        "BoundedSemaphore",
        "Barrier",
        "BrokenBarrierError",
        "Timer",
        "local",
        "current_thread",
        "main_thread",
        "get_ident",
        "get_native_id",
        "active_count",
        "enumerate",
        "TIMEOUT_MAX",
        "ExceptHookArgs",
        "excepthook",
        "settrace",
        "gettrace",
        "setprofile",
        "getprofile",
        "stack_size",
        "currentThread",
        "activeCount",
    ]

    # Intrinsics validated above; assign directly to eliminate 54 _require_callable
    # function-call + f-string IR sequences.
    _thread_join = _MOLT_THREAD_JOIN
    _thread_is_alive = _MOLT_THREAD_IS_ALIVE
    _thread_ident = _MOLT_THREAD_IDENT
    _thread_native_id = _MOLT_THREAD_NATIVE_ID
    _thread_current_ident = _MOLT_THREAD_CURRENT_IDENT
    _thread_current_native_id = _MOLT_THREAD_CURRENT_NATIVE_ID
    _thread_spawn_shared = _MOLT_THREAD_SPAWN_SHARED
    _thread_drop = _MOLT_THREAD_DROP
    _thread_stack_size_get = _MOLT_THREAD_STACK_SIZE_GET
    _thread_stack_size_set = _MOLT_THREAD_STACK_SIZE_SET
    _thread_registry_set_main = _MOLT_THREAD_REGISTRY_SET_MAIN
    _thread_registry_register = _MOLT_THREAD_REGISTRY_REGISTER
    _thread_registry_forget = _MOLT_THREAD_REGISTRY_FORGET
    _thread_registry_snapshot = _MOLT_THREAD_REGISTRY_SNAPSHOT
    _thread_registry_current = _MOLT_THREAD_REGISTRY_CURRENT
    _lock_new = _MOLT_LOCK_NEW
    _lock_acquire = _MOLT_LOCK_ACQUIRE
    _lock_release = _MOLT_LOCK_RELEASE
    _lock_locked = _MOLT_LOCK_LOCKED
    _lock_drop = _MOLT_LOCK_DROP
    _rlock_new = _MOLT_RLOCK_NEW
    _rlock_acquire = _MOLT_RLOCK_ACQUIRE
    _rlock_release = _MOLT_RLOCK_RELEASE
    _rlock_locked = _MOLT_RLOCK_LOCKED
    _rlock_is_owned = _MOLT_RLOCK_IS_OWNED
    _rlock_release_save = _MOLT_RLOCK_RELEASE_SAVE
    _rlock_acquire_restore = _MOLT_RLOCK_ACQUIRE_RESTORE
    _rlock_drop = _MOLT_RLOCK_DROP
    _condition_new = _MOLT_CONDITION_NEW
    _condition_wait = _MOLT_CONDITION_WAIT
    _condition_wait_for = _MOLT_CONDITION_WAIT_FOR
    _condition_notify = _MOLT_CONDITION_NOTIFY
    _condition_drop = _MOLT_CONDITION_DROP
    _event_new = _MOLT_EVENT_NEW
    _event_set = _MOLT_EVENT_SET
    _event_clear = _MOLT_EVENT_CLEAR
    _event_is_set = _MOLT_EVENT_IS_SET
    _event_wait = _MOLT_EVENT_WAIT
    _event_drop = _MOLT_EVENT_DROP
    _semaphore_new = _MOLT_SEMAPHORE_NEW
    _semaphore_acquire = _MOLT_SEMAPHORE_ACQUIRE
    _semaphore_release = _MOLT_SEMAPHORE_RELEASE
    _semaphore_drop = _MOLT_SEMAPHORE_DROP
    _barrier_new = _MOLT_BARRIER_NEW
    _barrier_wait = _MOLT_BARRIER_WAIT
    _barrier_abort = _MOLT_BARRIER_ABORT
    _barrier_reset = _MOLT_BARRIER_RESET
    _barrier_parties = _MOLT_BARRIER_PARTIES
    _barrier_n_waiting = _MOLT_BARRIER_N_WAITING
    _barrier_broken = _MOLT_BARRIER_BROKEN
    _barrier_drop = _MOLT_BARRIER_DROP
    _local_new = _MOLT_LOCAL_NEW
    _local_get_dict = _MOLT_LOCAL_GET_DICT
    _local_drop = _MOLT_LOCAL_DROP

    _THREAD_COUNTER = 0
    _THREAD_TOKEN_COUNTER = 0
    _MAIN_THREAD: "Thread" | None = None

    _TRACE_HOOK: Callable[[Any, str, Any], Any] | None = None
    _PROFILE_HOOK: Callable[[Any, str, Any], Any] | None = None
    _NO_CONTEXT = object()

    TIMEOUT_MAX = 9223372036.0

    class ExceptHookArgs:
        def __init__(
            self,
            exc_type: type[BaseException],
            exc_value: BaseException,
            exc_traceback: Any,
            thread: "Thread",
        ) -> None:
            self.exc_type = exc_type
            self.exc_value = exc_value
            self.exc_traceback = exc_traceback
            self.thread = thread

    def _default_excepthook(args: ExceptHookArgs) -> None:
        print_exception = getattr(sys, "__excepthook__", None)
        if not callable(print_exception):
            print_exception = getattr(sys, "excepthook", None)
        if not callable(print_exception):
            raise RuntimeError("sys.excepthook unavailable")
        print("Exception in thread " + str(args.thread.name) + ":", file=sys.stderr)
        print_exception(args.exc_type, args.exc_value, args.exc_traceback)

    excepthook: Callable[[ExceptHookArgs], Any] | None = _default_excepthook

    def settrace(func: Callable[[Any, str, Any], Any] | None) -> None:
        global _TRACE_HOOK
        _TRACE_HOOK = func

    def gettrace() -> Callable[[Any, str, Any], Any] | None:
        return _TRACE_HOOK

    def setprofile(func: Callable[[Any, str, Any], Any] | None) -> None:
        global _PROFILE_HOOK
        _PROFILE_HOOK = func

    def getprofile() -> Callable[[Any, str, Any], Any] | None:
        return _PROFILE_HOOK

    def stack_size(size: int | None = None) -> int:
        if size is None:
            return int(_thread_stack_size_get())
        if not isinstance(size, int):
            raise TypeError(
                "'" + type(size).__name__ + "' object cannot be interpreted as an integer"
            )
        new_size = int(size)
        return int(_thread_stack_size_set(new_size))

    def _next_thread_name() -> str:
        global _THREAD_COUNTER
        _THREAD_COUNTER += 1
        return "Thread-" + str(_THREAD_COUNTER)

    def _next_thread_token() -> int:
        global _THREAD_TOKEN_COUNTER
        _THREAD_TOKEN_COUNTER += 1
        return _THREAD_TOKEN_COUNTER

    def get_ident() -> int:
        return _expect_int(_thread_current_ident())

    def get_native_id() -> int:
        return _expect_int(_thread_current_native_id())

    def _check_timeout_max(timeout_val: float) -> None:
        if timeout_val > TIMEOUT_MAX:
            raise OverflowError("timestamp out of range for platform time_t")

    _BAD_TIMEOUT_MSG = "' object cannot be interpreted as an integer or float"

    def _validate_lock_timeout(timeout: float, blocking: bool) -> float:
        """Validate timeout for Lock/RLock acquire."""
        if timeout is None:
            raise TypeError("'NoneType" + _BAD_TIMEOUT_MSG)
        try:
            timeout_val = float(timeout)
        except (TypeError, ValueError) as exc:
            raise TypeError(
                "'" + type(timeout).__name__ + _BAD_TIMEOUT_MSG
            ) from exc
        if not blocking:
            if timeout_val != -1.0:
                raise ValueError("can't specify a timeout for a non-blocking call")
        elif timeout_val < 0.0 and timeout_val != -1.0:
            raise ValueError("timeout value must be a non-negative number")
        if blocking and timeout_val != -1.0:
            _check_timeout_max(timeout_val)
        return timeout_val

    def _validate_wait_timeout(timeout: float | None) -> float | None:
        """Validate optional timeout for wait-style methods."""
        if timeout is None:
            return None
        try:
            timeout_val = float(timeout)
        except (TypeError, ValueError) as exc:
            raise TypeError(
                "'" + type(timeout).__name__ + _BAD_TIMEOUT_MSG
            ) from exc
        if timeout_val < 0.0:
            timeout_val = 0.0
        _check_timeout_max(timeout_val)
        return timeout_val

    def _invoke_thread_hooks() -> None:
        if _TRACE_HOOK is not None:
            try:
                _TRACE_HOOK(None, "call", None)
            except Exception:
                pass
        if _PROFILE_HOOK is not None:
            try:
                _PROFILE_HOOK(None, "call", None)
            except Exception:
                pass

    def _call_excepthook(thread: "Thread", exc: BaseException) -> None:
        hook = excepthook
        if hook is None:
            return
        args = ExceptHookArgs(type(exc), exc, exc.__traceback__, thread)
        try:
            hook(args)
        except Exception:
            try:
                _default_excepthook(args)
            except Exception:
                pass

    _R_NAME = 0
    _R_DAEMON = 1
    _R_IDENT = 2
    _R_NATIVE_ID = 3
    _R_ALIVE = 4
    _R_IS_MAIN = 5

    def _parse_registry_record(record: Any) -> list[Any]:
        """Parse a 6-element registry record into a list (avoids tuple unpack)."""
        if not isinstance(record, tuple) or len(record) != 6:
            raise RuntimeError("invalid thread registry record")
        return [
            str(record[0]), bool(record[1]),
            None if record[2] is None else int(record[2]),
            None if record[3] is None else int(record[3]),
            bool(record[4]), bool(record[5]),
        ]

    def _apply_record_to_thread(rec: list[Any], thread: "Thread") -> None:
        """Apply parsed registry record fields to a Thread object."""
        thread._name = rec[_R_NAME]
        thread._daemon = rec[_R_DAEMON]
        thread._started = bool(rec[_R_ALIVE])
        thread._synthetic_alive = bool(rec[_R_ALIVE])
        thread._ident_cache = rec[_R_IDENT]
        thread._native_id_cache = rec[_R_NATIVE_ID]

    def _thread_from_registry_record(record: Any) -> "Thread":
        rec = _parse_registry_record(record)
        thread = Thread(target=None, name=rec[_R_NAME], daemon=rec[_R_DAEMON])
        thread._started = rec[_R_ALIVE]
        thread._synthetic_alive = rec[_R_ALIVE]
        thread._ident_cache = rec[_R_IDENT]
        thread._native_id_cache = rec[_R_NATIVE_ID]
        thread._handle = None
        return thread

    class Lock:
        def __init__(self) -> None:
            self._handle: Any | None = _lock_new()

        def acquire(self, blocking: bool = True, timeout: float = -1.0) -> bool:
            timeout_val = _validate_lock_timeout(timeout, blocking)
            if self._handle is None:
                raise RuntimeError("lock is not initialized")
            return bool(_lock_acquire(self._handle, bool(blocking), timeout_val))

        def release(self) -> None:
            if self._handle is None:
                raise RuntimeError("lock is not initialized")
            _lock_release(self._handle)

        def locked(self) -> bool:
            if self._handle is None:
                return False
            return bool(_lock_locked(self._handle))

        def _is_owned(self) -> bool:
            return self.locked()

        def _release_save(self) -> None:
            self.release()

        def _acquire_restore(self, _state: Any) -> None:
            self.acquire()

        def __enter__(self) -> Lock:
            self.acquire()
            return self

        def __exit__(self, _exc_type: Any, _exc: Any, _tb: Any) -> bool:
            self.release()
            return False

        def _drop(self) -> None:
            if self._handle is None:
                return
            _lock_drop(self._handle)
            self._handle = None

        def __del__(self) -> None:
            if getattr(self, "_handle", None) is None:
                return
            self._drop()

    class RLock:
        def __init__(self) -> None:
            self._handle: Any | None = _rlock_new()

        def acquire(self, blocking: bool = True, timeout: float = -1.0) -> bool:
            timeout_val = _validate_lock_timeout(timeout, blocking)
            if self._handle is None:
                raise RuntimeError("rlock is not initialized")
            return bool(_rlock_acquire(self._handle, bool(blocking), timeout_val))

        def release(self) -> None:
            if self._handle is None:
                raise RuntimeError("rlock is not initialized")
            _rlock_release(self._handle)

        def _is_owned(self) -> bool:
            if self._handle is None:
                return False
            return bool(_rlock_is_owned(self._handle))

        def _release_save(self) -> int:
            if self._handle is None:
                raise RuntimeError("rlock is not initialized")
            return int(_rlock_release_save(self._handle))

        def _acquire_restore(self, count: int | None) -> None:
            if self._handle is None:
                raise RuntimeError("rlock is not initialized")
            if count is None:
                _rlock_acquire_restore(self._handle, 1)
                return
            _rlock_acquire_restore(self._handle, int(count))

        def __enter__(self) -> RLock:
            self.acquire()
            return self

        def __exit__(self, _exc_type: Any, _exc: Any, _tb: Any) -> bool:
            self.release()
            return False

        def _drop(self) -> None:
            if self._handle is None:
                return
            _rlock_drop(self._handle)
            self._handle = None

        def __del__(self) -> None:
            if getattr(self, "_handle", None) is None:
                return
            self._drop()

    class Condition:
        def __init__(self, lock: Any | None = None) -> None:
            if lock is None:
                lock = RLock()
            self._lock = lock
            self._handle: Any | None = _condition_new()

        def acquire(self, *args: Any, **kwargs: Any) -> bool:
            return bool(self._lock.acquire(*args, **kwargs))

        def release(self) -> None:
            self._lock.release()

        def __enter__(self) -> Condition:
            self.acquire()
            return self

        def __exit__(self, _exc_type: Any, _exc: Any, _tb: Any) -> bool:
            self.release()
            return False

        def _is_owned(self) -> bool:
            if hasattr(self._lock, "_is_owned"):
                return bool(self._lock._is_owned())
            acquired = self._lock.acquire(False)
            if acquired:
                self._lock.release()
                return False
            return True

        def _release_save(self) -> Any:
            if hasattr(self._lock, "_release_save"):
                return self._lock._release_save()
            self._lock.release()
            return None

        def _acquire_restore(self, state: Any) -> None:
            if hasattr(self._lock, "_acquire_restore"):
                self._lock._acquire_restore(state)
                return
            self._lock.acquire()

        def wait(self, timeout: float | None = None) -> bool:
            if not self._is_owned():
                raise RuntimeError("cannot wait on un-acquired lock")
            timeout_val = _validate_wait_timeout(timeout)
            if self._handle is None:
                raise RuntimeError("condition is not initialized")
            saved = self._release_save()
            try:
                return bool(_condition_wait(self._handle, timeout_val))
            finally:
                self._acquire_restore(saved)

        def wait_for(
            self, predicate: Callable[[], bool], timeout: float | None = None
        ) -> bool:
            timeout_val = None if timeout is None else float(timeout)
            return bool(_condition_wait_for(self, predicate, timeout_val))

        def notify(self, n: int = 1) -> None:
            if not self._is_owned():
                raise RuntimeError("cannot notify on un-acquired lock")
            if self._handle is None:
                raise RuntimeError("condition is not initialized")
            if n <= 0:
                return
            _condition_notify(self._handle, int(n))

        def notify_all(self) -> None:
            self.notify(1 << 30)

        notifyAll = notify_all

        def _drop(self) -> None:
            if self._handle is None:
                return
            _condition_drop(self._handle)
            self._handle = None

        def __del__(self) -> None:
            if getattr(self, "_handle", None) is None:
                return
            self._drop()

    class Event:
        def __init__(self) -> None:
            self._handle: Any | None = _event_new()

        def is_set(self) -> bool:
            if self._handle is None:
                return False
            return bool(_event_is_set(self._handle))

        def set(self) -> None:
            if self._handle is None:
                raise RuntimeError("event is not initialized")
            _event_set(self._handle)

        def clear(self) -> None:
            if self._handle is None:
                raise RuntimeError("event is not initialized")
            _event_clear(self._handle)

        def wait(self, timeout: float | None = None) -> bool:
            if self._handle is None:
                raise RuntimeError("event is not initialized")
            timeout_val = _validate_wait_timeout(timeout)
            return bool(_event_wait(self._handle, timeout_val))

        isSet = is_set

        def _drop(self) -> None:
            if self._handle is None:
                return
            _event_drop(self._handle)
            self._handle = None

        def __del__(self) -> None:
            if getattr(self, "_handle", None) is None:
                return
            self._drop()

    class Semaphore:
        def __init__(self, value: int = 1) -> None:
            if value < 0:
                raise ValueError("semaphore initial value must be >= 0")
            self._handle: Any | None = _semaphore_new(int(value), False)

        def acquire(self, blocking: bool = True, timeout: float | None = None) -> bool:
            if self._handle is None:
                raise RuntimeError("semaphore is not initialized")
            timeout_val = _validate_wait_timeout(timeout)
            if not blocking:
                if timeout_val is not None:
                    raise ValueError("can't specify timeout for non-blocking acquire")
                timeout_val = 0.0
            return bool(_semaphore_acquire(self._handle, bool(blocking), timeout_val))

        def release(self, n: int = 1) -> None:
            if self._handle is None:
                raise RuntimeError("semaphore is not initialized")
            if n < 1:
                raise ValueError("semaphore release count must be >= 1")
            _semaphore_release(self._handle, int(n))

        def __enter__(self) -> Semaphore:
            self.acquire()
            return self

        def __exit__(self, _exc_type: Any, _exc: Any, _tb: Any) -> bool:
            self.release()
            return False

        def _drop(self) -> None:
            if self._handle is None:
                return
            _semaphore_drop(self._handle)
            self._handle = None

        def __del__(self) -> None:
            if getattr(self, "_handle", None) is None:
                return
            self._drop()

    class BoundedSemaphore(Semaphore):
        def __init__(self, value: int = 1) -> None:
            if value < 0:
                raise ValueError("semaphore initial value must be >= 0")
            self._handle: Any | None = _semaphore_new(int(value), True)

    class BrokenBarrierError(RuntimeError):
        pass

    class Barrier:
        def __init__(
            self,
            parties: int,
            action: Callable[[], Any] | None = None,
            timeout: float | None = None,
        ) -> None:
            parties = int(parties)
            if parties <= 0:
                raise ValueError("barrier parties must be greater than zero")
            self._action = action
            timeout_val = _validate_wait_timeout(timeout)
            self._handle: Any | None = _barrier_new(parties, timeout_val)

        @property
        def parties(self) -> int:
            if self._handle is None:
                raise RuntimeError("barrier is not initialized")
            return int(_barrier_parties(self._handle))

        @property
        def n_waiting(self) -> int:
            if self._handle is None:
                raise RuntimeError("barrier is not initialized")
            return int(_barrier_n_waiting(self._handle))

        @property
        def broken(self) -> bool:
            if self._handle is None:
                raise RuntimeError("barrier is not initialized")
            return bool(_barrier_broken(self._handle))

        def abort(self) -> None:
            if self._handle is None:
                raise RuntimeError("barrier is not initialized")
            _barrier_abort(self._handle)

        def reset(self) -> None:
            if self._handle is None:
                raise RuntimeError("barrier is not initialized")
            _barrier_reset(self._handle)

        def wait(self, timeout: float | None = None) -> int:
            if self._handle is None:
                raise RuntimeError("barrier is not initialized")
            timeout_val = _validate_wait_timeout(timeout)
            try:
                index = int(_barrier_wait(self._handle, timeout_val))
            except RuntimeError as exc:
                if "broken barrier" in str(exc):
                    raise BrokenBarrierError from None
                raise
            if self._action is not None and index == self.parties - 1:
                try:
                    self._action()
                except Exception:
                    self.abort()
                    raise
            return index

        def _drop(self) -> None:
            if self._handle is None:
                return
            _barrier_drop(self._handle)
            self._handle = None

        def __del__(self) -> None:
            if getattr(self, "_handle", None) is None:
                return
            self._drop()

    class local:
        def __init__(self) -> None:
            object.__setattr__(self, "_handle", _local_new())

        def _get_dict(self) -> dict[str, Any]:
            handle = object.__getattribute__(self, "_handle")
            if handle is None:
                raise RuntimeError("thread-local storage is not initialized")
            return _local_get_dict(handle)

        @property
        def __dict__(self) -> dict[str, Any]:
            return self._get_dict()

        def __getattr__(self, name: str) -> Any:
            data = self._get_dict()
            try:
                return data[name]
            except KeyError as exc:
                raise AttributeError(name) from exc

        def __setattr__(self, name: str, value: Any) -> None:
            if name == "_handle":
                object.__setattr__(self, name, value)
                return
            data = self._get_dict()
            data[name] = value

        def __delattr__(self, name: str) -> None:
            data = self._get_dict()
            if name not in data:
                raise AttributeError(name)
            del data[name]

        def _drop(self) -> None:
            handle = object.__getattribute__(self, "_handle")
            if handle is None:
                return
            _local_drop(handle)
            object.__setattr__(self, "_handle", None)

        def __del__(self) -> None:
            try:
                self._drop()
            except Exception:
                pass

    class Thread:
        def __init__(
            self,
            _group: Any | None = None,
            target: Callable[..., Any] | None = None,
            name: str | None = None,
            args: tuple[Any, ...] = (),
            kwargs: dict[str, Any] | None = None,
            *,
            daemon: bool | None = None,
            context: Any = _NO_CONTEXT,
        ) -> None:
            if _group is not None:
                raise ValueError("group argument must be None for now")
            if context is not _NO_CONTEXT:
                raise TypeError(
                    "Thread.__init__() got an unexpected keyword argument 'context'"
                )
            self._target = target
            self._args = tuple(args)
            self._kwargs = dict(kwargs) if kwargs else {}
            self._handle: Any | None = None
            self._started = False
            self._ident_cache: int | None = None
            self._native_id_cache: int | None = None
            self._synthetic_alive: bool | None = None
            self._token: int | None = None
            if daemon is not None:
                self._daemon = bool(daemon)
            elif _MAIN_THREAD is None:
                self._daemon = False
            else:
                self._daemon = current_thread().daemon
            self._name = name or _next_thread_name()

        def __repr__(self) -> str:
            status = "started" if self._started else "initial"
            return "<Thread " + self._name + " (" + status + ")>"

        @property
        def name(self) -> str:
            return self._name

        @name.setter
        def name(self, value: str) -> None:
            if self._started:
                raise RuntimeError("cannot set name after start")
            self._name = str(value)

        @property
        def daemon(self) -> bool:
            return self._daemon

        @daemon.setter
        def daemon(self, value: bool) -> None:
            if self._started:
                raise RuntimeError("cannot set daemon after start")
            self._daemon = bool(value)

        @property
        def ident(self) -> int | None:
            if self._ident_cache is not None:
                return self._ident_cache
            if not self._started or self._handle is None:
                return None
            ident = _thread_ident(self._handle)
            self._ident_cache = _expect_int(ident) if ident is not None else None
            return self._ident_cache

        @property
        def native_id(self) -> int | None:
            if self._native_id_cache is not None:
                return self._native_id_cache
            if not self._started or self._handle is None:
                return None
            nid = _thread_native_id(self._handle)
            self._native_id_cache = _expect_int(nid) if nid is not None else None
            return self._native_id_cache

        def is_alive(self) -> bool:
            if not self._started:
                return False
            if self._handle is None:
                return bool(self._synthetic_alive)
            return bool(_thread_is_alive(self._handle))

        def start(self) -> None:
            if self._started:
                raise RuntimeError("threads can only be started once")
            token = _next_thread_token()
            self._token = token
            handle = _thread_spawn_shared(token, self._bootstrap, (), {})
            self._handle = handle
            self._started = True
            self._synthetic_alive = True
            _thread_registry_register(self._handle, token, self._name, self._daemon)
            ident = _thread_ident(self._handle)
            if ident is not None:
                self._ident_cache = _expect_int(ident)
            native = _thread_native_id(self._handle)
            if native is not None:
                self._native_id_cache = _expect_int(native)

        def join(self, timeout: float | None = None) -> None:
            if not self._started:
                raise RuntimeError("cannot join thread before it is started")
            ident = self.ident
            if ident is not None and ident == get_ident():
                raise RuntimeError("cannot join current thread")
            if self._handle is None:
                return None
            if timeout is None:
                _thread_join(self._handle, None)
                return None
            timeout_val = _validate_wait_timeout(timeout)
            _thread_join(self._handle, timeout_val)

        def run(self) -> None:
            if self._target is None:
                return None
            self._target(*self._args, **self._kwargs)

        def _bootstrap(self) -> None:
            try:
                _invoke_thread_hooks()
                self.run()
            except BaseException as exc:
                _call_excepthook(self, exc)
            finally:
                self._synthetic_alive = False

        def _drop(self) -> None:
            if self._token is not None:
                _thread_registry_forget(self._token)
            if self._handle is None:
                self._synthetic_alive = False
                return
            _thread_drop(self._handle)
            self._handle = None
            self._synthetic_alive = False

        def __del__(self) -> None:
            if getattr(self, "_handle", None) is None:
                return
            self._drop()

        def setDaemon(self, value: bool) -> None:
            self.daemon = value

        def isDaemon(self) -> bool:
            return self.daemon

        def getName(self) -> str:
            return self.name

        def setName(self, name: str) -> None:
            self.name = name

    class Timer(Thread):
        def __init__(
            self,
            interval: float,
            function: Callable[..., Any],
            args: tuple[Any, ...] | None = None,
            kwargs: dict[str, Any] | None = None,
        ) -> None:
            super().__init__(target=None)
            self.interval = float(interval)
            self.function = function
            self.args = tuple(args) if args else ()
            self.kwargs = dict(kwargs) if kwargs else {}
            self.finished = Event()

        def cancel(self) -> None:
            self.finished.set()

        def run(self) -> None:
            if not self.finished.wait(self.interval):
                self.function(*self.args, **self.kwargs)
            self.finished.set()

    def _registry_current_record() -> list[Any]:
        record = _thread_registry_current()
        if record is None:
            return ["MainThread", False, get_ident(), get_native_id(), True, True]
        return _parse_registry_record(record)

    def _registry_snapshot_records() -> list[list[Any]]:
        records = _thread_registry_snapshot()
        if not isinstance(records, list):
            raise RuntimeError("invalid thread registry snapshot")
        result: list[list[Any]] = []
        for record in records:
            result.append(_parse_registry_record(record))
        return result

    def current_thread() -> Thread:
        rec = _registry_current_record()
        if rec[_R_IS_MAIN]:
            thread = (
                _MAIN_THREAD if _MAIN_THREAD is not None else _bootstrap_main_thread()
            )
            _apply_record_to_thread(rec, thread)
            return thread
        return _thread_from_registry_record(
            (rec[_R_NAME], rec[_R_DAEMON], rec[_R_IDENT], rec[_R_NATIVE_ID], rec[_R_ALIVE], rec[_R_IS_MAIN])
        )

    def main_thread() -> Thread:
        if _MAIN_THREAD is None:
            return _bootstrap_main_thread()
        return _MAIN_THREAD

    def enumerate() -> list[Thread]:
        out: list[Thread] = []
        for rec in _registry_snapshot_records():
            if not rec[_R_ALIVE]:
                continue
            if rec[_R_IS_MAIN]:
                thread = main_thread()
                _apply_record_to_thread(rec, thread)
                out.append(thread)
                continue
            out.append(_thread_from_registry_record(
                (rec[_R_NAME], rec[_R_DAEMON], rec[_R_IDENT], rec[_R_NATIVE_ID], rec[_R_ALIVE], rec[_R_IS_MAIN])
            ))
        if not out:
            out.append(main_thread())
        return out

    def active_count() -> int:
        return int(_MOLT_THREAD_REGISTRY_ACTIVE_COUNT())

    currentThread = current_thread
    activeCount = active_count

    def _bootstrap_main_thread() -> Thread:
        global _MAIN_THREAD
        thread = Thread(target=None, name="MainThread", daemon=False)
        thread._started = True
        thread._synthetic_alive = True
        thread._ident_cache = get_ident()
        thread._native_id_cache = get_native_id()
        _thread_registry_set_main(str(thread._name or "MainThread"), bool(thread._daemon))
        _MAIN_THREAD = thread
        return thread

    _bootstrap_main_thread()

globals().pop("_require_intrinsic", None)
