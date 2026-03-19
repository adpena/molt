"""Intrinsic-backed _thread module for Molt.

Low-level thread primitives.  The higher-level ``threading`` module builds on
top of this module.  All behaviour is backed by Rust runtime intrinsics; no
CPython fallback is used.
"""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

# ---------------------------------------------------------------------------
# Load intrinsics (require_intrinsic raises RuntimeError if unavailable)
# ---------------------------------------------------------------------------

_lock_new = _require_intrinsic("molt_lock_new")
_lock_acquire = _require_intrinsic("molt_lock_acquire")
_lock_release = _require_intrinsic("molt_lock_release")
_lock_locked = _require_intrinsic("molt_lock_locked")
_lock_drop = _require_intrinsic("molt_lock_drop")

_thread_spawn_shared = _require_intrinsic("molt_thread_spawn_shared")
_thread_ident = _require_intrinsic("molt_thread_ident")
_thread_current_ident = _require_intrinsic("molt_thread_current_ident")
_thread_current_native_id = _require_intrinsic("molt_thread_current_native_id")
_thread_registry_active_count = _require_intrinsic("molt_thread_registry_active_count")
_thread_stack_size_get = _require_intrinsic("molt_thread_stack_size_get")
_thread_stack_size_set = _require_intrinsic("molt_thread_stack_size_set")
_signal_raise_signal = _require_intrinsic("molt_signal_raise_signal")

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

TIMEOUT_MAX: float = 9223372036.0
"""Maximum timeout value (~292 years), matching CPython's PY_TIMEOUT_MAX / 1e9."""

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
    "allocate",
    "allocate_lock",
    "error",
    "exit",
    "exit_thread",
    "get_ident",
    "get_native_id",
    "interrupt_main",
    "lock",
    "stack_size",
    "start_new",
    "start_new_thread",
]


# ---------------------------------------------------------------------------
# LockType
# ---------------------------------------------------------------------------


class LockType:
    """Low-level lock object backed by Molt runtime intrinsics."""

    def __init__(self, _lock_new_intrinsic=_lock_new) -> None:
        self._handle: Any | None = _lock_new_intrinsic()

    # -- acquire / release / locked -----------------------------------------

    def acquire(
        self,
        blocking: bool = True,
        timeout: float = -1.0,
        _lock_acquire_intrinsic=_lock_acquire,
    ) -> bool:
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
        return bool(
            _lock_acquire_intrinsic(self._handle, bool(blocking), timeout_val)
        )

    def release(self, _lock_release_intrinsic=_lock_release) -> None:
        """Release the lock."""
        if self._handle is None:
            raise RuntimeError("lock is not initialized")
        _lock_release_intrinsic(self._handle)

    def locked(self, _lock_locked_intrinsic=_lock_locked) -> bool:
        """Return whether the lock is currently held."""
        if self._handle is None:
            return False
        return bool(_lock_locked_intrinsic(self._handle))

    # -- context manager protocol -------------------------------------------

    def __enter__(self) -> bool:
        return self.acquire()

    def __exit__(self, _exc_type: Any, _exc: Any, _tb: Any) -> None:
        self.release()

    # -- handle lifecycle ---------------------------------------------------

    def _drop(self, _lock_drop_intrinsic=_lock_drop) -> None:
        if self._handle is None:
            return
        _lock_drop_intrinsic(self._handle)
        self._handle = None

    def __del__(self) -> None:
        if getattr(self, "_handle", None) is None:
            return
        self._drop()

    # -- repr ---------------------------------------------------------------

    def __repr__(self) -> str:
        status = "locked" if self.locked() else "unlocked"
        return f"<{status} _thread.lock object>"


LockType.__name__ = "lock"
LockType.__qualname__ = "lock"

# Alias: ``_thread.lock`` is the same type as ``_thread.LockType``.
lock = LockType


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
    function: Any,
    args: tuple[Any, ...] = (),
    kwargs: dict[str, Any] | None = None,
    _thread_spawn_shared_intrinsic=_thread_spawn_shared,
    _thread_ident_intrinsic=_thread_ident,
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
    handle = _thread_spawn_shared_intrinsic(token, function, args, kwargs)
    ident = _thread_ident_intrinsic(handle)
    if ident is not None:
        return int(ident)
    # Fallback: use the token as ident if the runtime cannot provide one yet.
    return token


# Aliases matching CPython's _thread module.
start_new = start_new_thread


def exit() -> None:
    """Raise ``SystemExit`` to exit the current thread."""
    raise SystemExit


# Alias matching CPython's _thread module.
exit_thread = exit


def get_ident(_thread_current_ident_intrinsic=_thread_current_ident) -> int:
    """Return the thread identifier of the current thread."""
    return int(_thread_current_ident_intrinsic())


def get_native_id(_thread_current_native_id_intrinsic=_thread_current_native_id) -> int:
    """Return the native integral thread ID of the current thread."""
    return int(_thread_current_native_id_intrinsic())


def _count(_thread_registry_active_count_intrinsic=_thread_registry_active_count) -> int:
    """Return the number of currently active threads (including the main thread)."""
    return int(_thread_registry_active_count_intrinsic())


def stack_size(
    size: int = 0,
    _thread_stack_size_get_intrinsic=_thread_stack_size_get,
    _thread_stack_size_set_intrinsic=_thread_stack_size_set,
) -> int:
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
        return int(_thread_stack_size_get_intrinsic())
    return int(_thread_stack_size_set_intrinsic(size))


def interrupt_main(signum: int = 2, _signal_raise_signal_intrinsic=_signal_raise_signal) -> None:
    """Simulate the effect of a signal arriving in the main thread.

    The default *signum* is ``SIGINT`` (2).
    """
    _signal_raise_signal_intrinsic(int(signum))


# ---------------------------------------------------------------------------
# Namespace cleanup — remove names that are not part of CPython's _thread API.
# ---------------------------------------------------------------------------
for _name in ("Any",):
    globals().pop(_name, None)

for _name in (
    "_lock_new",
    "_lock_acquire",
    "_lock_release",
    "_lock_locked",
    "_lock_drop",
    "_thread_spawn_shared",
    "_thread_ident",
    "_thread_current_ident",
    "_thread_current_native_id",
    "_thread_registry_active_count",
    "_thread_stack_size_get",
    "_thread_stack_size_set",
    "_signal_raise_signal",
):
    globals().pop(_name, None)
