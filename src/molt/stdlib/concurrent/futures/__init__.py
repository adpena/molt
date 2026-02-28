"""Rust-intrinsic-backed concurrent.futures support for Molt."""

from __future__ import annotations

from collections import deque, namedtuple
from collections.abc import Callable, Iterable, Iterator
from dataclasses import dataclass
from typing import Any, TYPE_CHECKING
import os
import threading
import time

from _intrinsics import require_intrinsic as _require_intrinsic

from builtins import TimeoutError as _BuiltinTimeoutError


if TYPE_CHECKING:

    def molt_thread_submit(_func: Any, _args: Any, _kwargs: Any) -> Any: ...


_PENDING = "PENDING"
_RUNNING = "RUNNING"
_CANCELLED = "CANCELLED"
_FINISHED = "FINISHED"
_DONE_STATES = {_CANCELLED, _FINISHED}


class CancelledError(Exception):
    """Raised when a Future is cancelled."""


class TimeoutError(_BuiltinTimeoutError):
    """Raised when a Future result is not available in time."""


class InvalidStateError(Exception):
    """Raised when setting a Future state transition is invalid."""


class BrokenExecutor(RuntimeError):
    """Raised when an executor cannot accept new work."""


class BrokenThreadPool(BrokenExecutor):
    """Raised when ThreadPoolExecutor worker bootstrap fails."""


class BrokenProcessPool(BrokenExecutor):
    """Raised when ProcessPoolExecutor cannot schedule new work."""


FIRST_COMPLETED = "FIRST_COMPLETED"
FIRST_EXCEPTION = "FIRST_EXCEPTION"
ALL_COMPLETED = "ALL_COMPLETED"

_DoneAndNotDone = namedtuple("DoneAndNotDone", "done not_done")

_MOLT_THREAD_SUBMIT = _require_intrinsic("molt_thread_submit", globals())
_MOLT_THREAD_SPAWN = _require_intrinsic("molt_thread_spawn", globals())


def _is_intrinsic(func: Any | None) -> bool:
    if not callable(func):
        return False
    return type(func).__name__ == "builtin_function_or_method"


_MOLT_THREADPOOL = _is_intrinsic(_MOLT_THREAD_SPAWN)


def _submit_thread_work(
    fn: Callable[..., Any],
    args: tuple[Any, ...],
    kwargs: dict[str, Any],
) -> Any:
    return _MOLT_THREAD_SUBMIT(fn, args, kwargs)


@dataclass(slots=True)
class _WorkItem:
    fn: Callable[..., Any]
    args: tuple[Any, ...]
    kwargs: dict[str, Any]
    future: "Future"


class Future:
    def __init__(self) -> None:
        self._condition = _threading.Condition()
        self._state = _PENDING
        self._result: Any | None = None
        self._exception: BaseException | None = None
        self._callbacks: list[Callable[[Future], Any]] = []

    def cancel(self) -> bool:
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
        with self._condition:
            return self._state == _CANCELLED

    def running(self) -> bool:
        with self._condition:
            return self._state == _RUNNING

    def done(self) -> bool:
        with self._condition:
            return self._state in _DONE_STATES

    def _wait_done_locked(self, timeout: float | None) -> bool:
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

    def result(self, timeout: float | None = None) -> Any:
        with self._condition:
            if not self._wait_done_locked(timeout):
                raise TimeoutError()
            if self._state == _CANCELLED:
                raise CancelledError()
            if self._exception is not None:
                raise self._exception
            return self._result

    def exception(self, timeout: float | None = None) -> BaseException | None:
        with self._condition:
            if not self._wait_done_locked(timeout):
                raise TimeoutError()
            if self._state == _CANCELLED:
                raise CancelledError()
            return self._exception

    def add_done_callback(self, fn: Callable[[Future], Any]) -> None:
        call_now = False
        with self._condition:
            if self._state in _DONE_STATES:
                call_now = True
            else:
                self._callbacks.append(fn)
        if call_now:
            self._invoke_callbacks([fn])

    def set_running_or_notify_cancel(self) -> bool:
        with self._condition:
            if self._state == _CANCELLED:
                return False
            if self._state != _PENDING:
                raise InvalidStateError("Future in unexpected state")
            self._state = _RUNNING
            return True

    def _set_running_or_notify_cancel(self) -> bool:
        return self.set_running_or_notify_cancel()

    def set_result(self, result: Any) -> None:
        callbacks: list[Callable[[Future], Any]] = []
        with self._condition:
            if self._state in _DONE_STATES:
                raise InvalidStateError("Future already done")
            if self._state == _CANCELLED:
                raise InvalidStateError("Future cancelled")
            self._result = result
            self._state = _FINISHED
            callbacks = list(self._callbacks)
            self._callbacks.clear()
            self._condition.notify_all()
        self._invoke_callbacks(callbacks)

    def set_exception(self, exc: BaseException) -> None:
        callbacks: list[Callable[[Future], Any]] = []
        with self._condition:
            if self._state in _DONE_STATES:
                raise InvalidStateError("Future already done")
            if self._state == _CANCELLED:
                raise InvalidStateError("Future cancelled")
            self._exception = exc
            self._state = _FINISHED
            callbacks = list(self._callbacks)
            self._callbacks.clear()
            self._condition.notify_all()
        self._invoke_callbacks(callbacks)

    def _invoke_callbacks(self, callbacks: list[Callable[[Future], Any]]) -> None:
        for cb in callbacks:
            try:
                cb(self)
            except Exception:
                pass

    def _has_exception(self) -> bool:
        with self._condition:
            return self._state == _FINISHED and self._exception is not None


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

        args_iter = iter(zip(*iterables))
        pending: list[Future] = []
        deadline = None if timeout is None else (_time.monotonic() + float(timeout))

        def _remaining() -> float | None:
            if deadline is None:
                return None
            rem = deadline - _time.monotonic()
            if rem <= 0:
                raise TimeoutError()
            return rem

        for args in args_iter:
            pending.append(self.submit(fn, *args))

        try:
            while pending:
                fut = pending.pop(0)
                yield fut.result(timeout=_remaining())
        finally:
            for fut in pending:
                fut.cancel()

    def __enter__(self) -> "Executor":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.shutdown(wait=True)


class ThreadPoolExecutor(Executor):
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

        self._max_workers = int(max_workers)
        self._thread_name_prefix = str(thread_name_prefix)
        self._initializer = initializer
        self._initargs = tuple(initargs)
        self._broken: BrokenThreadPool | None = None

        # Intrinsic pool runs through runtime threads and cannot guarantee per-thread
        # Python-level initializer semantics.
        self._molt_enabled = _MOLT_THREADPOOL and initializer is None
        self._molt_cancel_futures = False
        self._molt_inflight = 0
        self._molt_running = 0
        self._molt_lock = _threading.Lock()
        self._molt_done = _threading.Condition(self._molt_lock)
        self._molt_queue: deque[_WorkItem] = _deque()
        self._molt_futures: set[Future] = set()

        self._threads: list[threading.Thread] = []
        self._queue: deque[_WorkItem] = _deque()
        self._lock = _threading.Lock()
        self._work_ready = _threading.Condition(self._lock)
        self._shutdown = False

        if not self._molt_enabled:
            self._start_threads()

    def _start_threads(self) -> None:
        for idx in range(self._max_workers):
            name = f"ThreadPoolExecutor-{idx}"
            if self._thread_name_prefix:
                name = f"{self._thread_name_prefix}_{idx}"
            thread = _threading.Thread(
                target=_threadpool_worker_bootstrap,
                args=(self,),
                name=name,
                daemon=True,
            )
            thread.start()
            self._threads.append(thread)

    def _set_broken(self, exc: BaseException) -> None:
        broken = (
            exc
            if isinstance(exc, BrokenThreadPool)
            else BrokenThreadPool("ThreadPoolExecutor worker initialization failed")
        )
        if broken is not exc:
            broken.__cause__ = exc

        pending: list[_WorkItem] = []
        with self._work_ready:
            if self._broken is not None:
                return
            self._broken = broken
            pending.extend(self._queue)
            self._queue.clear()
            self._shutdown = True
            self._work_ready.notify_all()

        for item in pending:
            if not item.future.done():
                try:
                    item.future.set_exception(broken)
                except InvalidStateError:
                    pass

    def submit(self, fn: Callable[..., Any], /, *args: Any, **kwargs: Any) -> Future:
        if fn is None or not callable(fn):
            raise TypeError("submit expects a callable")

        future = Future()
        item = _WorkItem(fn=fn, args=tuple(args), kwargs=dict(kwargs), future=future)

        if self._molt_enabled:
            with self._molt_done:
                if self._shutdown:
                    raise RuntimeError("cannot schedule new futures after shutdown")
                if self._broken is not None:
                    raise self._broken
                self._molt_queue.append(item)
                self._molt_futures.add(future)
                self._molt_inflight += 1
                to_schedule = self._molt_drain_locked()
            for work_item in to_schedule:
                _submit_thread_work(_molt_threadpool_worker, (self, work_item), {})
            return future

        with self._work_ready:
            if self._shutdown:
                raise RuntimeError("cannot schedule new futures after shutdown")
            if self._broken is not None:
                raise self._broken
            self._queue.append(item)
            self._work_ready.notify()
        return future

    def shutdown(self, wait: bool = True, *, cancel_futures: bool = False) -> None:
        if self._molt_enabled:
            with self._molt_done:
                self._shutdown = True
                if cancel_futures:
                    self._molt_cancel_futures = True
                    self._molt_cancel_queued_locked()
                self._molt_done.notify_all()
                if wait:
                    while self._molt_inflight:
                        self._molt_done.wait()
            return

        with self._work_ready:
            self._shutdown = True
            if cancel_futures:
                while self._queue:
                    item = self._queue.popleft()
                    item.future.cancel()
            self._work_ready.notify_all()
        if wait:
            for thread in list(self._threads):
                thread.join()

    def _molt_task_done(self, future: Future) -> None:
        with self._molt_done:
            self._molt_futures.discard(future)
            if self._molt_running > 0:
                self._molt_running -= 1
            if self._molt_inflight > 0:
                self._molt_inflight -= 1
            to_schedule = self._molt_drain_locked()
            if self._molt_inflight == 0:
                self._molt_done.notify_all()
        for work_item in to_schedule:
            _submit_thread_work(_molt_threadpool_worker, (self, work_item), {})

    def _molt_drain_locked(self) -> list[_WorkItem]:
        to_schedule: list[_WorkItem] = []
        while self._molt_queue and self._molt_running < self._max_workers:
            item = self._molt_queue.popleft()
            if self._shutdown and self._molt_cancel_futures:
                item.future.cancel()
                self._molt_futures.discard(item.future)
                if self._molt_inflight > 0:
                    self._molt_inflight -= 1
                continue
            if item.future.cancelled():
                self._molt_futures.discard(item.future)
                if self._molt_inflight > 0:
                    self._molt_inflight -= 1
                continue
            if not item.future.set_running_or_notify_cancel():
                self._molt_futures.discard(item.future)
                if self._molt_inflight > 0:
                    self._molt_inflight -= 1
                continue
            self._molt_running += 1
            to_schedule.append(item)
        return to_schedule

    def _molt_cancel_queued_locked(self) -> None:
        while self._molt_queue:
            item = self._molt_queue.popleft()
            item.future.cancel()
            self._molt_futures.discard(item.future)
            if self._molt_inflight > 0:
                self._molt_inflight -= 1

    def _worker_bootstrap(self) -> None:
        if self._initializer is not None:
            try:
                self._initializer(*self._initargs)
            except BaseException as exc:  # noqa: BLE001
                self._set_broken(exc)
                return
        self._worker_loop()

    def _worker_loop(self) -> None:
        while True:
            with self._work_ready:
                while not self._queue and not self._shutdown:
                    self._work_ready.wait()
                if self._shutdown and not self._queue:
                    return
                item = self._queue.popleft()
            if not item.future.set_running_or_notify_cancel():
                continue
            try:
                result = item.fn(*item.args, **item.kwargs)
            except BaseException as exc:  # noqa: BLE001
                try:
                    item.future.set_exception(exc)
                except InvalidStateError:
                    pass
            else:
                try:
                    item.future.set_result(result)
                except InvalidStateError:
                    pass


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


def _threadpool_worker_bootstrap(executor: ThreadPoolExecutor) -> None:
    executor._worker_bootstrap()


def _molt_threadpool_worker(executor: ThreadPoolExecutor, item: _WorkItem) -> None:
    try:
        try:
            result = item.fn(*item.args, **item.kwargs)
        except BaseException as exc:  # noqa: BLE001
            try:
                item.future.set_exception(exc)
            except InvalidStateError:
                pass
        else:
            try:
                item.future.set_result(result)
            except InvalidStateError:
                pass
    finally:
        executor._molt_task_done(item.future)


def wait(
    fs: Iterable[Future],
    timeout: float | None = None,
    return_when: str = ALL_COMPLETED,
) -> tuple[set[Future], set[Future]]:
    futures = set(fs)
    if not futures:
        return _DoneAndNotDone(set(), set())
    if return_when not in (FIRST_COMPLETED, FIRST_EXCEPTION, ALL_COMPLETED):
        raise ValueError(
            "return_when must be FIRST_COMPLETED, FIRST_EXCEPTION, or ALL_COMPLETED"
        )

    done = {f for f in futures if f.done()}
    pending = set(futures - done)

    if return_when == FIRST_COMPLETED and done:
        return _DoneAndNotDone(done, pending)
    if return_when == FIRST_EXCEPTION:
        if any((not fut.cancelled()) and fut._has_exception() for fut in done):
            return _DoneAndNotDone(done, pending)
        if not pending:
            return _DoneAndNotDone(done, pending)
    if return_when == ALL_COMPLETED and not pending:
        return _DoneAndNotDone(done, pending)

    notifier = _threading.Condition()

    def _notify(_: Future) -> None:
        with notifier:
            notifier.notify_all()

    for fut in pending:
        fut.add_done_callback(_notify)

    deadline = None if timeout is None else (_time.monotonic() + float(timeout))
    while pending:
        if return_when == FIRST_COMPLETED and done:
            break
        if return_when == FIRST_EXCEPTION and any(
            (not fut.cancelled()) and fut._has_exception() for fut in done
        ):
            break
        if return_when == ALL_COMPLETED and not pending:
            break

        remaining = None
        if deadline is not None:
            remaining = deadline - _time.monotonic()
            if remaining <= 0:
                break
        with notifier:
            notifier.wait(timeout=remaining)

        done = {f for f in futures if f.done()}
        pending = set(futures - done)

    return _DoneAndNotDone(done, pending)


def as_completed(
    fs: Iterable[Future], timeout: float | None = None
) -> Iterator[Future]:
    futures = set(fs)
    if not futures:
        return iter(())
    pending = set(futures)
    deadline = None if timeout is None else (_time.monotonic() + float(timeout))

    while pending:
        remaining = None
        if deadline is not None:
            remaining = deadline - _time.monotonic()
            if remaining <= 0:
                raise TimeoutError()
        done, not_done = wait(pending, timeout=remaining, return_when=FIRST_COMPLETED)
        if not done:
            raise TimeoutError()
        pending = set(not_done)
        for fut in done:
            yield fut


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
# Namespace cleanup — remove names that are not part of CPython's
# concurrent.futures public API.  These are import-time helpers, typing
# aliases, or exception subclasses that CPython keeps in submodules.
# ---------------------------------------------------------------------------
# Preserve runtime-accessible aliases before removing public names.
_deque = deque
_threading = threading
_time = time
_os = os
for _name in (
    "deque",
    "namedtuple",
    "Callable",
    "Iterable",
    "Iterator",
    "Any",
    "TYPE_CHECKING",
    "dataclass",
    "os",
    "threading",
    "time",
):
    globals().pop(_name, None)
