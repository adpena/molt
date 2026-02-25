"""Intrinsic-backed _thread module for Molt.

Low-level thread primitives. The higher-level ``threading`` module builds on
top of this module.  All behaviour is backed by Rust runtime intrinsics; no
CPython fallback is used.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# ---------------------------------------------------------------------------
# Load intrinsics
# ---------------------------------------------------------------------------

_MOLT_LOCK_NEW = _require_intrinsic("molt_lock_new", globals())
_MOLT_LOCK_ACQUIRE = _require_intrinsic("molt_lock_acquire", globals())
_MOLT_LOCK_RELEASE = _require_intrinsic("molt_lock_release", globals())
_MOLT_LOCK_LOCKED = _require_intrinsic("molt_lock_locked", globals())
_MOLT_LOCK_DROP = _require_intrinsic("molt_lock_drop", globals())
_MOLT_THREAD_SPAWN_SHARED = _require_intrinsic("molt_thread_spawn_shared", globals())
_MOLT_THREAD_CURRENT_IDENT = _require_intrinsic("molt_thread_current_ident", globals())
_MOLT_THREAD_CURRENT_NATIVE_ID = _require_intrinsic(
    "molt_thread_current_native_id", globals()
)
_MOLT_THREAD_IDENT = _require_intrinsic("molt_thread_ident", globals())
_MOLT_THREAD_REGISTRY_ACTIVE_COUNT = _require_intrinsic(
    "molt_thread_registry_active_count", globals()
)
_MOLT_THREAD_STACK_SIZE_GET = _require_intrinsic(
    "molt_thread_stack_size_get", globals()
)
_MOLT_THREAD_STACK_SIZE_SET = _require_intrinsic(
    "molt_thread_stack_size_set", globals()
)
_MOLT_SIGNAL_RAISE_SIGNAL = _require_intrinsic("molt_signal_raise_signal", globals())

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

Any = object  # type: ignore[assignment]


def _require_callable(value: object, name: str) -> Any:
    if not callable(value):
        raise RuntimeError(f"{name} intrinsic unavailable")
    return value


def _expect_int(value: object) -> int:
    return int(value)


# Validate that all required intrinsics loaded successfully.
_lock_new = _require_callable(_MOLT_LOCK_NEW, "molt_lock_new")
_lock_acquire = _require_callable(_MOLT_LOCK_ACQUIRE, "molt_lock_acquire")
_lock_release = _require_callable(_MOLT_LOCK_RELEASE, "molt_lock_release")
_lock_locked = _require_callable(_MOLT_LOCK_LOCKED, "molt_lock_locked")
_lock_drop = _require_callable(_MOLT_LOCK_DROP, "molt_lock_drop")
_thread_spawn_shared = _require_callable(
    _MOLT_THREAD_SPAWN_SHARED, "molt_thread_spawn_shared"
)
_thread_current_ident = _require_callable(
    _MOLT_THREAD_CURRENT_IDENT, "molt_thread_current_ident"
)
_thread_current_native_id = _require_callable(
    _MOLT_THREAD_CURRENT_NATIVE_ID, "molt_thread_current_native_id"
)
_thread_ident = _require_callable(_MOLT_THREAD_IDENT, "molt_thread_ident")
_thread_registry_active_count = _require_callable(
    _MOLT_THREAD_REGISTRY_ACTIVE_COUNT, "molt_thread_registry_active_count"
)
_thread_stack_size_get = _require_callable(
    _MOLT_THREAD_STACK_SIZE_GET, "molt_thread_stack_size_get"
)
_thread_stack_size_set = _require_callable(
    _MOLT_THREAD_STACK_SIZE_SET, "molt_thread_stack_size_set"
)
_signal_raise_signal = _require_callable(
    _MOLT_SIGNAL_RAISE_SIGNAL, "molt_signal_raise_signal"
)

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

TIMEOUT_MAX: float = 4294967.0
"""Maximum timeout value (~49.7 days), matching CPython's _thread.TIMEOUT_MAX."""

error = RuntimeError
"""The standard _thread.error exception type (alias for RuntimeError)."""

# Internal token counter for thread spawning.
_THREAD_TOKEN_COUNTER: int = 0

# ---------------------------------------------------------------------------
# __all__
# ---------------------------------------------------------------------------

__all__ = [
    "LockType",
    "TIMEOUT_MAX",
    "_count",
    "allocate_lock",
    "error",
    "exit",
    "get_ident",
    "get_native_id",
    "interrupt_main",
    "stack_size",
    "start_new_thread",
]


# ---------------------------------------------------------------------------
# LockType
# ---------------------------------------------------------------------------


class LockType:
    """Low-level lock object backed by Molt runtime intrinsics."""

    def __init__(self) -> None:
        self._handle: Any | None = _lock_new()

    def acquire(self, blocking: bool = True, timeout: float = -1.0) -> bool:
        """Acquire the lock.

        *blocking* controls whether the call blocks.  *timeout* (seconds) is
        only meaningful when *blocking* is ``True``; a value of ``-1`` means
        wait forever.
        """
        if timeout is None:
            raise TypeError(
                "'NoneType' object cannot be interpreted as an integer or float"
            )
        try:
            timeout_val = float(timeout)
        except (TypeError, ValueError) as exc:
            raise TypeError(
                f"'{type(timeout).__name__}' object cannot be "
                f"interpreted as an integer or float"
            ) from exc
        if not blocking:
            if timeout_val != -1.0:
                raise ValueError("can't specify a timeout for a non-blocking call")
        elif timeout_val < 0.0 and timeout_val != -1.0:
            raise ValueError("timeout value must be a non-negative number")
        if blocking and timeout_val != -1.0:
            if timeout_val > TIMEOUT_MAX:
                raise OverflowError("timestamp out of range for platform time_t")
        if self._handle is None:
            raise RuntimeError("lock is not initialized")
        return bool(_lock_acquire(self._handle, bool(blocking), timeout_val))

    def release(self) -> None:
        """Release the lock."""
        if self._handle is None:
            raise RuntimeError("lock is not initialized")
        _lock_release(self._handle)

    def locked(self) -> bool:
        """Return whether the lock is currently held."""
        if self._handle is None:
            return False
        return bool(_lock_locked(self._handle))

    def __enter__(self) -> LockType:
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

    def __repr__(self) -> str:
        status = "locked" if self.locked() else "unlocked"
        return f"<_thread.lock object [{status}]>"


LockType.__name__ = "lock"
LockType.__qualname__ = "lock"


# ---------------------------------------------------------------------------
# Module-level functions
# ---------------------------------------------------------------------------


def allocate_lock() -> LockType:
    """Allocate a new lock object.  Equivalent to ``_thread.allocate_lock()``."""
    return LockType()


# Alias used by some CPython internals.
allocate = allocate_lock


def _next_thread_token() -> int:
    global _THREAD_TOKEN_COUNTER
    _THREAD_TOKEN_COUNTER += 1
    return _THREAD_TOKEN_COUNTER


def start_new_thread(
    function: Any, args: tuple[Any, ...] = (), kwargs: dict[str, Any] | None = None
) -> int:
    """Start a new thread and return its identifier.

    The thread executes *function(*args, **kwargs)*.  The return value is the
    thread identity (an integer).
    """
    if not callable(function):
        raise TypeError("first arg must be callable")
    if not isinstance(args, tuple):
        raise TypeError("2nd arg must be a tuple")
    if kwargs is not None and not isinstance(kwargs, dict):
        raise TypeError("3rd arg must be a dict")
    if kwargs is None:
        kwargs = {}
    token = _next_thread_token()
    handle = _thread_spawn_shared(token, function, args, kwargs)
    ident = _thread_ident(handle)
    if ident is not None:
        return _expect_int(ident)
    # Fallback: use the token as ident if the runtime cannot provide one yet.
    return token


def exit() -> None:
    """Raise ``SystemExit`` to exit the current thread."""
    raise SystemExit


def get_ident() -> int:
    """Return the thread identifier of the current thread."""
    return _expect_int(_thread_current_ident())


def get_native_id() -> int:
    """Return the native integral thread ID of the current thread."""
    return _expect_int(_thread_current_native_id())


def _count() -> int:
    """Return the number of currently active threads (including the main thread)."""
    return int(_thread_registry_active_count())


def stack_size(size: int = 0) -> int:
    """Get or set the thread stack size (in bytes).

    With no argument (or *size* == 0), return the current stack size.
    Otherwise, set the stack size for newly created threads and return the
    previous value.
    """
    if not isinstance(size, int):
        raise TypeError(
            f"'{type(size).__name__}' object cannot be interpreted as an integer"
        )
    if size == 0:
        return int(_thread_stack_size_get())
    return int(_thread_stack_size_set(size))


def interrupt_main(signum: int = 2) -> None:
    """Simulate the effect of a signal arriving in the main thread.

    The default *signum* is ``SIGINT`` (2).
    """
    _signal_raise_signal(int(signum))
