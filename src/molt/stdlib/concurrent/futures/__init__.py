"""Rust-intrinsic-backed concurrent.futures support for Molt.

All ThreadPoolExecutor concurrency state lives in the Rust runtime via
handle-based intrinsics.  Standalone Future objects (created directly by
user code) maintain Python-side state to support the full CPython API.

This module is a thin Python wrapper for argument normalization, error
mapping, and CPython API compatibility.
"""

from __future__ import annotations

from collections.abc import Callable, Iterable, Iterator
from collections import namedtuple
from typing import Any, TYPE_CHECKING
import os
import threading
import time

from _intrinsics import require_intrinsic as _require_intrinsic

from builtins import TimeoutError as _BuiltinTimeoutError

if TYPE_CHECKING:

    def molt_concurrent_threadpool_new(max_workers_bits: Any) -> Any: ...
    def molt_concurrent_threadpool_drop(handle_bits: Any) -> Any: ...
    def molt_concurrent_threadpool_submit(
        handle_bits: Any, fn_bits: Any, args_bits: Any
    ) -> Any: ...
    def molt_concurrent_threadpool_shutdown(
        handle_bits: Any, wait_bits: Any, _cancel_futures_bits: Any
    ) -> Any: ...
    def molt_concurrent_future_add_done_callback(
        handle_bits: Any, fn_bits: Any
    ) -> Any: ...
    def molt_concurrent_future_cancel(handle_bits: Any) -> Any: ...
    def molt_concurrent_future_cancelled(handle_bits: Any) -> Any: ...
    def molt_concurrent_future_done(handle_bits: Any) -> Any: ...
    def molt_concurrent_future_drop(handle_bits: Any) -> Any: ...
    def molt_concurrent_future_exception(
        handle_bits: Any, timeout_bits: Any
    ) -> Any: ...
    def molt_concurrent_future_result(handle_bits: Any, timeout_bits: Any) -> Any: ...
    def molt_concurrent_future_running(handle_bits: Any) -> Any: ...
    def molt_concurrent_wait(
        futures_bits: Any, timeout_bits: Any, return_when_bits: Any
    ) -> Any: ...
    def molt_concurrent_as_completed(futures_bits: Any, timeout_bits: Any) -> Any: ...
    def molt_concurrent_all_completed() -> Any: ...
    def molt_concurrent_first_completed() -> Any: ...
    def molt_concurrent_first_exception() -> Any: ...


# ---------------------------------------------------------------------------
# Load all intrinsics at module import time (hard-fail if unavailable).
# ---------------------------------------------------------------------------
_MOLT_THREADPOOL_NEW = _require_intrinsic("molt_concurrent_threadpool_new")
_MOLT_THREADPOOL_DROP = _require_intrinsic("molt_concurrent_threadpool_drop")
_MOLT_THREADPOOL_SUBMIT = _require_intrinsic(
    "molt_concurrent_threadpool_submit"
)
_MOLT_THREADPOOL_SHUTDOWN = _require_intrinsic(
    "molt_concurrent_threadpool_shutdown"
)
_MOLT_FUTURE_ADD_DONE_CALLBACK = _require_intrinsic(
    "molt_concurrent_future_add_done_callback"
)
_MOLT_FUTURE_CANCEL = _require_intrinsic("molt_concurrent_future_cancel")
_MOLT_FUTURE_CANCELLED = _require_intrinsic(
    "molt_concurrent_future_cancelled"
)
_MOLT_FUTURE_DONE = _require_intrinsic("molt_concurrent_future_done")
_MOLT_FUTURE_DROP = _require_intrinsic("molt_concurrent_future_drop")
_MOLT_FUTURE_EXCEPTION = _require_intrinsic(
    "molt_concurrent_future_exception"
)
_MOLT_FUTURE_RESULT = _require_intrinsic("molt_concurrent_future_result")
_MOLT_FUTURE_RUNNING = _require_intrinsic("molt_concurrent_future_running")
_MOLT_WAIT = _require_intrinsic("molt_concurrent_wait")
_MOLT_AS_COMPLETED = _require_intrinsic("molt_concurrent_as_completed")
_MOLT_ALL_COMPLETED = _require_intrinsic("molt_concurrent_all_completed")
_MOLT_FIRST_COMPLETED = _require_intrinsic("molt_concurrent_first_completed")
_MOLT_FIRST_EXCEPTION = _require_intrinsic("molt_concurrent_first_exception")


# ---------------------------------------------------------------------------
# Public constants (string values).
# ---------------------------------------------------------------------------
FIRST_COMPLETED = "FIRST_COMPLETED"
FIRST_EXCEPTION = "FIRST_EXCEPTION"
ALL_COMPLETED = "ALL_COMPLETED"

_DoneAndNotDone = namedtuple("DoneAndNotDone", "done not_done")

# Internal state string constants for Python-managed futures.
_PENDING = "PENDING"
_RUNNING = "RUNNING"
_CANCELLED = "CANCELLED"
_FINISHED = "FINISHED"
_DONE_STATES = frozenset((_CANCELLED, _FINISHED))


# ---------------------------------------------------------------------------
# Exceptions (match CPython's concurrent.futures hierarchy).
# ---------------------------------------------------------------------------


class CancelledError(Exception):
    """Raised when a Future is cancelled."""


class TimeoutError(_BuiltinTimeoutError):
    """Raised when a Future result is not available in time."""


class InvalidStateError(Exception):
    """Raised when a Future state transition is invalid."""


class BrokenExecutor(RuntimeError):
    """Raised when an executor cannot accept new work."""


class BrokenThreadPool(BrokenExecutor):
    """Raised when ThreadPoolExecutor worker bootstrap fails."""


class BrokenProcessPool(BrokenExecutor):
    """Raised when ProcessPoolExecutor cannot schedule new work."""


# ---------------------------------------------------------------------------
# Process-wide Future handle registry.
#
# The Rust wait/as_completed intrinsics return handle integers.  To map
# those back to Python Future objects we maintain a global dict keyed by
# the integer handle.  Entries are removed when the Future is dropped.
# Access is always under the GIL; no locking needed.
# ---------------------------------------------------------------------------

_FUTURE_REGISTRY: dict = {}  # int handle -> Future instance


# ---------------------------------------------------------------------------
# Future
# ---------------------------------------------------------------------------


class Future:
    """Thread-safe future result container.

    Futures created by ``ThreadPoolExecutor.submit()`` are backed by a Rust
    handle and delegate state queries to the runtime.

    Futures created directly (``Future()``) maintain Python-side state for
    full CPython API compatibility (``set_result``, ``set_exception``,
    ``set_running_or_notify_cancel``).
    """

    # No __slots__: Python-managed futures need __dict__ for _condition etc.

    def __init__(self) -> None:
        """Create a standalone Python-managed future."""
        # Rust handle; None means this future is Python-managed.
        self._handle: int | None = None

        # Python-managed state (only used when _handle is None).
        self._condition = _threading.Condition()
        self._state: str = _PENDING
        self._result: Any = None
        self._exception: BaseException | None = None
        self._callbacks: list[Callable[[Future], Any]] = []

    @classmethod
    def _from_handle(cls, handle: int) -> "Future":
        """Create a future that wraps a Rust runtime handle."""
        self = cls.__new__(cls)
        self._handle = handle
        return self

    # -- inspection ----------------------------------------------------------

    def cancel(self) -> bool:
        """Request cancellation.  Returns True if successfully cancelled."""
        if self._handle is not None:
            return bool(_MOLT_FUTURE_CANCEL(self._handle))
        # Python-managed path.
        callbacks: list[Callable[[Future], Any]] = []
        with self._condition:
            if self._state != _PENDING:
                return False
            self._state = _CANCELLED
            callbacks = list(self._callbacks)
            self._callbacks.clear()
            self._condition.notify_all()
        self._invoke_callbacks(callbacks)
        return True

    def cancelled(self) -> bool:
        """Return True if the future was cancelled."""
        if self._handle is not None:
            return bool(_MOLT_FUTURE_CANCELLED(self._handle))
        with self._condition:
            return self._state == _CANCELLED

    def running(self) -> bool:
        """Return True if the future is currently executing."""
        if self._handle is not None:
            return bool(_MOLT_FUTURE_RUNNING(self._handle))
        with self._condition:
            return self._state == _RUNNING

    def done(self) -> bool:
        """Return True if the future completed (any terminal state)."""
        if self._handle is not None:
            return bool(_MOLT_FUTURE_DONE(self._handle))
        with self._condition:
            return self._state in _DONE_STATES

    # -- results -------------------------------------------------------------

    def result(self, timeout: float | None = None) -> Any:
        """Block until complete and return the result.

        Raises CancelledError if cancelled, TimeoutError if timed out,
        or re-raises the stored exception.
        """
        if self._handle is not None:
            try:
                return _MOLT_FUTURE_RESULT(self._handle, timeout)
            except CancelledError:
                raise
            except TimeoutError:
                raise
            except Exception:
                raise

        # Python-managed path.
        with self._condition:
            if not self._wait_done_locked(timeout):
                raise TimeoutError()
            if self._state == _CANCELLED:
                raise CancelledError()
            if self._exception is not None:
                raise self._exception
            return self._result

    def exception(self, timeout: float | None = None) -> BaseException | None:
        """Return the exception raised by the callable, or None.

        Raises CancelledError if cancelled, TimeoutError if timed out.
        """
        if self._handle is not None:
            try:
                raw = _MOLT_FUTURE_EXCEPTION(self._handle, timeout)
            except CancelledError:
                raise
            except TimeoutError:
                raise
            if raw is None:
                return None
            # Rust returns a str for exceptions; wrap so callers get an object.
            if isinstance(raw, str):
                return Exception(raw)
            return raw  # type: ignore[return-value]

        # Python-managed path.
        with self._condition:
            if not self._wait_done_locked(timeout):
                raise TimeoutError()
            if self._state == _CANCELLED:
                raise CancelledError()
            return self._exception

    # -- callbacks -----------------------------------------------------------

    def add_done_callback(self, fn: Callable[["Future"], Any]) -> None:
        """Register a callback to be called when this future completes.

        If the future is already done the callback is invoked immediately.
        """
        if self._handle is not None:
            handle = self._handle
            future_self = self

            def _callback_wrapper(_handle_arg: int) -> None:
                f = _FUTURE_REGISTRY.get(handle, future_self)
                try:
                    fn(f)
                except Exception:
                    pass

            _MOLT_FUTURE_ADD_DONE_CALLBACK(self._handle, _callback_wrapper)
            return

        # Python-managed path.
        call_now = False
        with self._condition:
            if self._state in _DONE_STATES:
                call_now = True
            else:
                self._callbacks.append(fn)
        if call_now:
            self._invoke_callbacks([fn])

    # -- CPython internal API (used by executor implementations) -------------

    def set_running_or_notify_cancel(self) -> bool:
        """Transition PENDING -> RUNNING.

        Returns False and fires callbacks if the future was cancelled.
        Only valid on Python-managed futures (not Rust-backed).
        """
        if self._handle is not None:
            raise InvalidStateError(
                "set_running_or_notify_cancel is not supported on Rust-backed futures"
            )
        callbacks: list[Callable[[Future], Any]] = []
        with self._condition:
            if self._state == _CANCELLED:
                callbacks = list(self._callbacks)
                self._callbacks.clear()
                self._condition.notify_all()
                # Fire callbacks outside the lock.
                result = False
            elif self._state == _PENDING:
                self._state = _RUNNING
                result = True
            else:
                raise InvalidStateError(f"Future in unexpected state: {self._state!r}")
        if not result:
            self._invoke_callbacks(callbacks)
        return result

    # Alias used by older internal code.
    _set_running_or_notify_cancel = set_running_or_notify_cancel

    def set_result(self, result: Any) -> None:
        """Store result and mark the future as finished.

        Only valid on Python-managed futures (not Rust-backed).
        """
        if self._handle is not None:
            raise InvalidStateError(
                "set_result is not supported on Rust-backed futures"
            )
        callbacks: list[Callable[[Future], Any]] = []
        with self._condition:
            if self._state in _DONE_STATES:
                raise InvalidStateError(
                    f"Future already in terminal state: {self._state!r}"
                )
            self._result = result
            self._state = _FINISHED
            callbacks = list(self._callbacks)
            self._callbacks.clear()
            self._condition.notify_all()
        self._invoke_callbacks(callbacks)

    def set_exception(self, exc: BaseException) -> None:
        """Store an exception and mark the future as finished.

        Only valid on Python-managed futures (not Rust-backed).
        """
        if self._handle is not None:
            raise InvalidStateError(
                "set_exception is not supported on Rust-backed futures"
            )
        callbacks: list[Callable[[Future], Any]] = []
        with self._condition:
            if self._state in _DONE_STATES:
                raise InvalidStateError(
                    f"Future already in terminal state: {self._state!r}"
                )
            self._exception = exc
            self._state = _FINISHED
            callbacks = list(self._callbacks)
            self._callbacks.clear()
            self._condition.notify_all()
        self._invoke_callbacks(callbacks)

    # -- internal helpers ----------------------------------------------------

    def _wait_done_locked(self, timeout: float | None) -> bool:
        """Wait until done or timeout; must be called under self._condition."""
        if self._state in _DONE_STATES:
            return True
        if timeout is None:
            while self._state not in _DONE_STATES:
                self._condition.wait()
            return True
        end = _time.monotonic() + float(timeout)
        while self._state not in _DONE_STATES:
            remaining = end - _time.monotonic()
            if remaining <= 0:
                return False
            self._condition.wait(remaining)
        return True

    def _invoke_callbacks(self, callbacks: list[Callable[["Future"], Any]]) -> None:
        for cb in callbacks:
            try:
                cb(self)
            except Exception:
                pass

    def _has_exception(self) -> bool:
        """True if finished with an exception (non-blocking, used by wait())."""
        if self._handle is not None:
            # Only query exception state if the future is already done —
            # calling _MOLT_FUTURE_EXCEPTION on a pending future would block.
            if not _MOLT_FUTURE_DONE(self._handle):
                return False
            raw = _MOLT_FUTURE_EXCEPTION(self._handle, 0.0)
            return raw is not None
        with self._condition:
            return self._state == _FINISHED and self._exception is not None

    # -- lifecycle -----------------------------------------------------------

    def __del__(self) -> None:
        handle = self._handle
        if handle is not None:
            _FUTURE_REGISTRY.pop(handle, None)
            try:
                _MOLT_FUTURE_DROP(handle)
            except Exception:
                pass

    def __repr__(self) -> str:
        if self._handle is not None:
            state = (
                "cancelled"
                if self.cancelled()
                else "running"
                if self.running()
                else "finished"
                if self.done()
                else "pending"
            )
        else:
            with self._condition:
                state = self._state.lower()
        return f"<Future at {id(self):#x} state={state}>"


# ---------------------------------------------------------------------------
# Executor base class
# ---------------------------------------------------------------------------


class Executor:
    def submit(self, fn: Callable[..., Any], /, *args: Any, **kwargs: Any) -> Future:
        raise NotImplementedError()

    def shutdown(self, wait: bool = True, *, cancel_futures: bool = False) -> None:
        raise NotImplementedError()

    def map(
        self,
        fn: Callable[..., Any],
        *iterables: Iterable[Any],
        timeout: float | None = None,
        chunksize: int = 1,
    ) -> Iterator[Any]:
        if not iterables:
            raise TypeError("Executor.map() must have at least one iterable")
        if chunksize < 1:
            raise ValueError("chunksize must be >= 1")

        deadline = None if timeout is None else (_time.monotonic() + float(timeout))

        def _remaining() -> float | None:
            if deadline is None:
                return None
            rem = deadline - _time.monotonic()
            if rem <= 0:
                raise TimeoutError()
            return rem

        pending: list[Future] = []
        for args_tuple in zip(*iterables):
            pending.append(self.submit(fn, *args_tuple))

        try:
            while pending:
                fut = pending.pop(0)
                yield fut.result(timeout=_remaining())
        finally:
            for fut in pending:
                fut.cancel()

    def __enter__(self) -> "Executor":
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.shutdown(wait=True)


# ---------------------------------------------------------------------------
# ThreadPoolExecutor
# ---------------------------------------------------------------------------


class ThreadPoolExecutor(Executor):
    """Thread pool backed entirely by Rust intrinsics.

    All concurrency state is stored in the Rust runtime.  Python is only
    responsible for argument normalisation, error mapping, and lifecycle
    management.
    """

    def __init__(
        self,
        max_workers: int | None = None,
        thread_name_prefix: str = "",
        initializer: Callable[..., Any] | None = None,
        initargs: tuple[Any, ...] | list[Any] = (),
    ) -> None:
        if max_workers is None:
            max_workers = min(32, (_os.cpu_count() or 1) + 4)
        if max_workers <= 0:
            raise ValueError("max_workers must be greater than 0")
        if initializer is not None and not callable(initializer):
            raise TypeError("initializer must be a callable")
        if initializer is not None:
            # Rust workers cannot run a Python initializer per-thread.
            # TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:partial):
            # Support per-thread initializer in ThreadPoolExecutor via Rust intrinsic.
            raise NotImplementedError(
                "ThreadPoolExecutor: per-thread initializer is not supported "
                "in Molt compiled binaries (no CPython runtime)"
            )

        self._max_workers: int = int(max_workers)
        self._thread_name_prefix: str = str(thread_name_prefix)
        self._shutdown: bool = False
        self._handle: int | None = None  # set after successful pool creation

        handle = _MOLT_THREADPOOL_NEW(self._max_workers)
        if not isinstance(handle, int):
            raise BrokenThreadPool(
                "molt_concurrent_threadpool_new returned unexpected value"
            )
        self._handle = handle

    def submit(self, fn: Callable[..., Any], /, *args: Any, **kwargs: Any) -> Future:
        if self._shutdown:
            raise RuntimeError("cannot schedule new futures after shutdown")
        if fn is None or not callable(fn):
            raise TypeError("submit expects a callable")

        # The Rust concurrent worker calls fn_bits(args_bits) via
        # call_callable1 — i.e., fn(packed_args) with the args tuple as a
        # single positional argument.  We always wrap the user callable in a
        # shim that receives the packed tuple and unpacks it.
        _fn = fn
        _pos = tuple(args)
        _kw = dict(kwargs) if kwargs else {}

        if _kw:

            def _shim(_packed: tuple[Any, ...]) -> Any:
                return _fn(*_packed, **_kw)
        else:

            def _shim(_packed: tuple[Any, ...]) -> Any:
                return _fn(*_packed)

        future_handle = _MOLT_THREADPOOL_SUBMIT(self._handle, _shim, _pos)

        if not isinstance(future_handle, int):
            raise RuntimeError(
                "molt_concurrent_threadpool_submit returned unexpected value"
            )

        f = Future._from_handle(future_handle)
        _FUTURE_REGISTRY[future_handle] = f
        return f

    def shutdown(self, wait: bool = True, *, cancel_futures: bool = False) -> None:
        if self._shutdown:
            return
        self._shutdown = True
        if self._handle is not None:
            _MOLT_THREADPOOL_SHUTDOWN(self._handle, wait, cancel_futures)

    def __del__(self) -> None:
        try:
            shutdown = self._shutdown
            handle = self._handle
        except AttributeError:
            return
        if not shutdown and handle is not None:
            try:
                _MOLT_THREADPOOL_DROP(handle)
            except Exception:
                pass

    def __repr__(self) -> str:
        return f"<ThreadPoolExecutor at {id(self):#x} max_workers={self._max_workers}>"


# ---------------------------------------------------------------------------
# ProcessPoolExecutor — thread-based shim (fork not supported in Molt)
# ---------------------------------------------------------------------------


class ProcessPoolExecutor(ThreadPoolExecutor):
    """ProcessPoolExecutor implemented as a thread-based executor.

    Molt compiles Python to native/WASM binaries and does not support
    fork() or process-based multiprocessing.  This shim provides full
    API compatibility for code that uses ProcessPoolExecutor by
    delegating to the thread-based executor.
    """

    def __init__(
        self,
        max_workers: int | None = None,
        mp_context: Any | None = None,
        initializer: Callable[..., Any] | None = None,
        initargs: tuple[Any, ...] | list[Any] = (),
        *,
        max_tasks_per_child: int | None = None,
    ) -> None:
        # Validate process-specific arguments for API compatibility,
        # then silently ignore them since we run threads, not processes.
        if max_tasks_per_child is not None:
            if not isinstance(max_tasks_per_child, int) or max_tasks_per_child <= 0:
                raise ValueError("max_tasks_per_child must be a positive int or None")
        # mp_context is not meaningful in thread-based execution; ignore.
        # max_tasks_per_child is not meaningful in thread-based execution; ignore.

        if max_workers is None:
            max_workers = _os.cpu_count() or 1

        super().__init__(
            max_workers=max_workers,
            initializer=initializer,
            initargs=initargs,
        )


# ---------------------------------------------------------------------------
# wait()
# ---------------------------------------------------------------------------


def wait(
    fs: Iterable[Future],
    timeout: float | None = None,
    return_when: str = ALL_COMPLETED,
) -> tuple[set[Future], set[Future]]:
    """Wait for futures to complete.

    Returns a named 2-tuple (done, not_done) of Future sets.
    """
    futures_list = list(fs)
    if not futures_list:
        return _DoneAndNotDone(set(), set())
    if return_when not in (FIRST_COMPLETED, FIRST_EXCEPTION, ALL_COMPLETED):
        raise ValueError(
            "return_when must be FIRST_COMPLETED, FIRST_EXCEPTION, or ALL_COMPLETED"
        )

    # Partition into Rust-backed and Python-managed futures.
    rust_futures = [f for f in futures_list if f._handle is not None]
    py_futures = [f for f in futures_list if f._handle is None]

    if rust_futures and not py_futures:
        # Fast path: all futures are Rust-backed.
        handles = [f._handle for f in rust_futures]
        handle_to_future = {f._handle: f for f in rust_futures}
        done_handles, not_done_handles = _MOLT_WAIT(handles, timeout, return_when)
        done: set[Future] = set()
        for h in done_handles:
            f = handle_to_future.get(h)
            if f is not None:
                done.add(f)
        not_done: set[Future] = set()
        for h in not_done_handles:
            f = handle_to_future.get(h)
            if f is not None:
                not_done.add(f)
        return _DoneAndNotDone(done, not_done)

    # Mixed or Python-only path: poll with Python-side logic.
    done_set = {f for f in futures_list if f.done()}
    pending_set = set(futures_list) - done_set

    if return_when == FIRST_COMPLETED and done_set:
        return _DoneAndNotDone(done_set, pending_set)
    if return_when == FIRST_EXCEPTION:
        if any((not f.cancelled()) and f._has_exception() for f in done_set):
            return _DoneAndNotDone(done_set, pending_set)
        if not pending_set:
            return _DoneAndNotDone(done_set, pending_set)
    if return_when == ALL_COMPLETED and not pending_set:
        return _DoneAndNotDone(done_set, pending_set)

    notifier = _threading.Condition()

    def _notify(_: Future) -> None:
        with notifier:
            notifier.notify_all()

    for f in pending_set:
        f.add_done_callback(_notify)

    deadline = None if timeout is None else (_time.monotonic() + float(timeout))
    while pending_set:
        if return_when == FIRST_COMPLETED and done_set:
            break
        if return_when == FIRST_EXCEPTION and any(
            (not f.cancelled()) and f._has_exception() for f in done_set
        ):
            break
        if return_when == ALL_COMPLETED and not pending_set:
            break

        remaining = None
        if deadline is not None:
            remaining = deadline - _time.monotonic()
            if remaining <= 0:
                break
        with notifier:
            notifier.wait(timeout=remaining)

        done_set = {f for f in futures_list if f.done()}
        pending_set = set(futures_list) - done_set

    return _DoneAndNotDone(done_set, pending_set)


# ---------------------------------------------------------------------------
# as_completed()
# ---------------------------------------------------------------------------


def as_completed(
    fs: Iterable[Future], timeout: float | None = None
) -> Iterator[Future]:
    """Yield futures as they complete.

    Raises TimeoutError if the timeout expires before all futures complete.
    """
    futures_list = list(fs)
    if not futures_list:
        return

    # Partition into Rust-backed and Python-managed futures.
    rust_futures = [f for f in futures_list if f._handle is not None]
    py_futures = [f for f in futures_list if f._handle is None]

    if rust_futures and not py_futures:
        # Fast path: all Rust-backed; delegate to the Rust intrinsic.
        handles = [f._handle for f in rust_futures]
        handle_to_future = {f._handle: f for f in rust_futures}
        completed_handles = _MOLT_AS_COMPLETED(handles, timeout)
        yielded = 0
        for h in completed_handles:
            f = handle_to_future.get(h)
            if f is not None:
                if f.done():
                    yielded += 1
                    yield f
                else:
                    # Still pending — timeout was hit.
                    raise TimeoutError(
                        f"as_completed timed out with "
                        f"{len(futures_list) - yielded} futures still pending"
                    )
        return

    # Mixed or Python-only path: poll with Python-side logic.
    pending = set(futures_list)
    deadline = None if timeout is None else (_time.monotonic() + float(timeout))

    while pending:
        remaining = None
        if deadline is not None:
            remaining = deadline - _time.monotonic()
            if remaining <= 0:
                raise TimeoutError()
        done, not_done = wait(
            list(pending), timeout=remaining, return_when=FIRST_COMPLETED
        )
        if not done:
            raise TimeoutError()
        pending = not_done
        for f in done:
            yield f


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------

__all__ = [
    "ALL_COMPLETED",
    "BrokenExecutor",
    "BrokenProcessPool",
    "BrokenThreadPool",
    "CancelledError",
    "Executor",
    "FIRST_COMPLETED",
    "FIRST_EXCEPTION",
    "Future",
    "InvalidStateError",
    "ProcessPoolExecutor",
    "ThreadPoolExecutor",
    "TimeoutError",
    "as_completed",
    "wait",
]


# ---------------------------------------------------------------------------
# Namespace cleanup — preserve runtime-accessible aliases before removing
# public names that are not part of CPython's concurrent.futures public API.
# ---------------------------------------------------------------------------
_threading = threading
_time = time
_os = os
for _name in (
    "namedtuple",
    "Callable",
    "Iterable",
    "Iterator",
    "Any",
    "TYPE_CHECKING",
    "os",
    "threading",
    "time",
):
    globals().pop(_name, None)

globals().pop("_require_intrinsic", None)
