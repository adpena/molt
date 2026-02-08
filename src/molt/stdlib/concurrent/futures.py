"""Lightweight concurrent.futures implementation for Molt.

TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial):
Implement ProcessPoolExecutor, executor shutdown cancel_futures parity, and full Future API
callbacks/cancellation semantics.
"""

from __future__ import annotations

import abc as _abc
from collections import deque
from collections.abc import Callable, Iterable, Iterator
from typing import Any, TYPE_CHECKING
import os
import threading
import time

from _intrinsics import require_intrinsic as _require_intrinsic


from builtins import TimeoutError as _BuiltinTimeoutError


if TYPE_CHECKING:

    def molt_thread_submit(_func: Any, _args: Any, _kwargs: Any) -> Any: ...


class CancelledError(Exception):
    """Raised when a Future is cancelled."""


class TimeoutError(_BuiltinTimeoutError):
    """Raised when a Future result is not available in time."""


FIRST_COMPLETED = object()
FIRST_EXCEPTION = object()
ALL_COMPLETED = object()

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


def _molt_threadpool_worker(
    executor: "ThreadPoolExecutor",
    future: "Future",
    fn: Callable[..., Any],
    args: tuple[Any, ...],
    kwargs: dict[str, Any],
) -> None:
    try:
        if executor._shutdown and executor._molt_cancel_futures:
            future.cancel()
            return
        if not future._set_running_or_notify_cancel():
            return
        try:
            result = fn(*args, **kwargs)
        except BaseException as exc:  # noqa: BLE001 - propagate into future
            future.set_exception(exc)
        else:
            future.set_result(result)
    finally:
        executor._molt_task_done(future)


class Future:
    def __init__(self) -> None:
        self._condition = threading.Condition()
        self._done = False
        self._running = False
        self._cancelled = False
        self._result: Any | None = None
        self._exception: BaseException | None = None
        self._callbacks: list[Callable[[Future], Any]] = []

    def cancel(self) -> bool:
        with self._condition:
            if self._done:
                return False
            self._cancelled = True
            self._done = True
            self._condition.notify_all()
        self._invoke_callbacks()
        return True

    def cancelled(self) -> bool:
        return self._cancelled

    def running(self) -> bool:
        return self._running and not self._done

    def done(self) -> bool:
        return self._done

    def _wait_done(self, timeout: float | None) -> bool:
        if self._done:
            return True
        if timeout is None:
            while not self._done:
                self._condition.wait()
            return True
        end = time.monotonic() + float(timeout)
        while not self._done:
            remaining = end - time.monotonic()
            if remaining <= 0:
                return False
            self._condition.wait(remaining)
        return True

    def result(self, timeout: float | None = None) -> Any:
        self._condition.acquire()
        try:
            if not self._done:
                if not self._wait_done(timeout):
                    raise TimeoutError()
            if self._cancelled:
                raise CancelledError()
            if self._exception is not None:
                raise self._exception
            return self._result
        finally:
            self._condition.release()

    def exception(self, timeout: float | None = None) -> BaseException | None:
        self._condition.acquire()
        try:
            if not self._done:
                if not self._wait_done(timeout):
                    raise TimeoutError()
            return self._exception
        finally:
            self._condition.release()

    def add_done_callback(self, fn: Callable[[Future], Any]) -> None:
        call_now = False
        with self._condition:
            if self._done:
                call_now = True
            else:
                self._callbacks.append(fn)
        if call_now:
            try:
                fn(self)
            except Exception:
                pass

    def set_result(self, result: Any) -> None:
        with self._condition:
            if self._done:
                return
            self._result = result
            self._done = True
            self._condition.notify_all()
        self._invoke_callbacks()

    def set_exception(self, exc: BaseException) -> None:
        with self._condition:
            if self._done:
                return
            self._exception = exc
            self._done = True
            self._condition.notify_all()
        self._invoke_callbacks()

    def _set_running_or_notify_cancel(self) -> bool:
        with self._condition:
            if self._cancelled:
                return False
            self._running = True
            return True

    def _invoke_callbacks(self) -> None:
        callbacks: list[Callable[[Future], Any]]
        with self._condition:
            callbacks = list(self._callbacks)
            self._callbacks.clear()
        for cb in callbacks:
            try:
                cb(self)
            except Exception:
                pass


class Executor(_abc.ABC):
    @_abc.abstractmethod
    def submit(
        self, fn: Callable[..., Any], /, *args: Any, **kwargs: Any
    ) -> Future: ...

    @_abc.abstractmethod
    def shutdown(self, wait: bool = True, *, cancel_futures: bool = False) -> None: ...

    def __enter__(self) -> "Executor":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.shutdown(wait=True)


class ThreadPoolExecutor(Executor):
    def __init__(self, max_workers: int | None = None) -> None:
        if max_workers is None:
            max_workers = os.cpu_count() or 1
        if max_workers <= 0:
            raise ValueError("max_workers must be greater than 0")
        self._max_workers = max_workers
        self._molt_enabled = _MOLT_THREADPOOL
        self._molt_cancel_futures = False
        self._molt_inflight = 0
        self._molt_lock = threading.Lock()
        self._molt_done = threading.Condition(self._molt_lock)
        self._molt_futures: set[Future] = set()
        self._molt_running = 0
        self._molt_queue: deque[
            tuple[Callable[..., Any], tuple[Any, ...], dict[str, Any], Future]
        ] = deque()
        self._threads: list[threading.Thread] = []
        self._queue: deque[
            tuple[Callable[..., Any], tuple[Any, ...], dict[str, Any], Future]
        ] = deque()
        self._lock = threading.Lock()
        self._work_ready = threading.Condition(self._lock)
        self._shutdown = False
        if not self._molt_enabled:
            self._start_threads()

    def _start_threads(self) -> None:
        for idx in range(self._max_workers):
            thread = threading.Thread(
                target=_threadpool_worker,
                args=(self,),
                name=f"ThreadPoolExecutor-{idx}",
                daemon=True,
            )
            thread.start()
            self._threads.append(thread)

    def submit(self, fn: Callable[..., Any], /, *args: Any, **kwargs: Any) -> Future:
        if fn is None:
            raise TypeError("submit expects a callable")
        future = Future()
        if self._molt_enabled:
            with self._molt_done:
                if self._shutdown:
                    raise RuntimeError("cannot schedule new futures after shutdown")
                self._molt_futures.add(future)
                self._molt_inflight += 1
                self._molt_queue.append((fn, args, kwargs, future))
                to_schedule = self._molt_drain_locked()
            for item in to_schedule:
                _submit_thread_work(_molt_threadpool_worker, (self, *item), {})
            return future
        with self._work_ready:
            if self._shutdown:
                raise RuntimeError("cannot schedule new futures after shutdown")
            self._queue.append((fn, args, kwargs, future))
            self._work_ready.notify()
        return future

    def shutdown(self, wait: bool = True, *, cancel_futures: bool = False) -> None:
        if self._molt_enabled:
            with self._molt_done:
                self._shutdown = True
                if cancel_futures:
                    self._molt_cancel_futures = True
                    for fut in list(self._molt_futures):
                        try:
                            fut.cancel()
                        except Exception:
                            pass
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
                    _, _, _, fut = self._queue.popleft()
                    fut.cancel()
            self._work_ready.notify_all()
        if wait:
            for thread in list(self._threads):
                thread.join()

    def _molt_task_done(self, future: Future) -> None:
        if not self._molt_enabled:
            return
        with self._molt_done:
            if future in self._molt_futures:
                self._molt_futures.discard(future)
            if self._molt_running > 0:
                self._molt_running -= 1
            if self._molt_inflight > 0:
                self._molt_inflight -= 1
            to_schedule = self._molt_drain_locked()
            if self._molt_inflight == 0:
                self._molt_done.notify_all()
        for item in to_schedule:
            _submit_thread_work(_molt_threadpool_worker, (self, *item), {})

    def _molt_drain_locked(
        self,
    ) -> list[tuple[Future, Callable[..., Any], tuple[Any, ...], dict[str, Any]]]:
        to_schedule: list[
            tuple[Future, Callable[..., Any], tuple[Any, ...], dict[str, Any]]
        ] = []
        while self._molt_queue and self._molt_running < self._max_workers:
            fn, args, kwargs, future = self._molt_queue.popleft()
            if self._shutdown and self._molt_cancel_futures:
                future.cancel()
                if future in self._molt_futures:
                    self._molt_futures.discard(future)
                if self._molt_inflight > 0:
                    self._molt_inflight -= 1
                continue
            if future.cancelled():
                if future in self._molt_futures:
                    self._molt_futures.discard(future)
                if self._molt_inflight > 0:
                    self._molt_inflight -= 1
                continue
            self._molt_running += 1
            to_schedule.append((future, fn, args, kwargs))
        return to_schedule

    def _molt_cancel_queued_locked(self) -> None:
        remaining: deque[
            tuple[Callable[..., Any], tuple[Any, ...], dict[str, Any], Future]
        ] = deque()
        while self._molt_queue:
            fn, args, kwargs, future = self._molt_queue.popleft()
            try:
                future.cancel()
            except Exception:
                remaining.append((fn, args, kwargs, future))
                continue
            if future in self._molt_futures:
                self._molt_futures.discard(future)
            if self._molt_inflight > 0:
                self._molt_inflight -= 1
        self._molt_queue = remaining

    def _worker(self) -> None:
        while True:
            with self._work_ready:
                while not self._queue and not self._shutdown:
                    self._work_ready.wait()
                if self._shutdown and not self._queue:
                    return
                fn, args, kwargs, future = self._queue.popleft()
            if not future._set_running_or_notify_cancel():
                continue
            try:
                result = fn(*args, **kwargs)
            except BaseException as exc:  # noqa: BLE001 - propagate into future
                future.set_exception(exc)
            else:
                future.set_result(result)


def _threadpool_worker(executor: ThreadPoolExecutor) -> None:
    executor._worker()


def wait(
    fs: Iterable[Future],
    timeout: float | None = None,
    return_when: object = ALL_COMPLETED,
) -> tuple[set[Future], set[Future]]:
    futures = set(fs)
    if not futures:
        return set(), set()
    if return_when not in (FIRST_COMPLETED, FIRST_EXCEPTION, ALL_COMPLETED):
        raise ValueError(
            "return_when must be FIRST_COMPLETED, FIRST_EXCEPTION, or ALL_COMPLETED"
        )

    done: set[Future] = set()
    pending: set[Future] = set(futures)
    notifier = threading.Condition()

    def _notify(_: Future) -> None:
        with notifier:
            notifier.notify_all()

    for fut in futures:
        fut.add_done_callback(_notify)

    def _update_done() -> bool:
        triggered = False
        for fut in list(pending):
            if fut.done():
                pending.remove(fut)
                done.add(fut)
                if return_when is FIRST_COMPLETED:
                    triggered = True
                elif return_when is FIRST_EXCEPTION:
                    if fut.cancelled() or fut.exception() is not None:
                        triggered = True
        return triggered

    deadline = None if timeout is None else (time.monotonic() + timeout)
    while pending:
        if _update_done() and return_when is not ALL_COMPLETED:
            break
        if not pending:
            break
        remaining = None
        if deadline is not None:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
        with notifier:
            notifier.wait(timeout=remaining)

    _update_done()
    return done, pending


def as_completed(
    fs: Iterable[Future], timeout: float | None = None
) -> Iterator[Future]:
    futures = set(fs)
    if not futures:
        return iter(())
    deadline = None if timeout is None else (time.monotonic() + timeout)
    pending = set(futures)
    while pending:
        remaining = None
        if deadline is not None:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError()
        done, pending = wait(pending, timeout=remaining, return_when=FIRST_COMPLETED)
        for fut in done:
            yield fut


__all__ = [
    "ALL_COMPLETED",
    "CancelledError",
    "Executor",
    "FIRST_COMPLETED",
    "FIRST_EXCEPTION",
    "Future",
    "ThreadPoolExecutor",
    "TimeoutError",
    "as_completed",
    "wait",
]
