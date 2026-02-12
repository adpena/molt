"""Rust-intrinsic-backed concurrent.futures support for Molt."""

from __future__ import annotations

import abc as _abc
from collections import deque, namedtuple
from collections.abc import Callable, Iterable, Iterator
from dataclasses import dataclass
from typing import Any, TYPE_CHECKING
import multiprocessing as _molt_multiprocessing
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


@dataclass(slots=True)
class _ProcessTask:
    fn: Callable[..., Any]
    args: tuple[Any, ...]
    kwargs: dict[str, Any]
    future: "Future"
    async_result: Any | None = None


class Future:
    def __init__(self) -> None:
        self._condition = threading.Condition()
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
        end = time.monotonic() + float(timeout)
        while self._state not in _DONE_STATES:
            remaining = end - time.monotonic()
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


class Executor(_abc.ABC):
    @_abc.abstractmethod
    def submit(
        self, fn: Callable[..., Any], /, *args: Any, **kwargs: Any
    ) -> Future: ...

    @_abc.abstractmethod
    def shutdown(self, wait: bool = True, *, cancel_futures: bool = False) -> None: ...

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
        pending: deque[Future] = deque()
        deadline = None if timeout is None else (time.monotonic() + float(timeout))

        def _remaining() -> float | None:
            if deadline is None:
                return None
            rem = deadline - time.monotonic()
            if rem <= 0:
                raise TimeoutError()
            return rem

        for args in args_iter:
            pending.append(self.submit(fn, *args))

        try:
            while pending:
                fut = pending.popleft()
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
            max_workers = min(32, (os.cpu_count() or 1) + 4)
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
        self._molt_lock = threading.Lock()
        self._molt_done = threading.Condition(self._molt_lock)
        self._molt_queue: deque[_WorkItem] = deque()
        self._molt_futures: set[Future] = set()

        self._threads: list[threading.Thread] = []
        self._queue: deque[_WorkItem] = deque()
        self._lock = threading.Lock()
        self._work_ready = threading.Condition(self._lock)
        self._shutdown = False

        if not self._molt_enabled:
            self._start_threads()

    def _start_threads(self) -> None:
        for idx in range(self._max_workers):
            name = f"ThreadPoolExecutor-{idx}"
            if self._thread_name_prefix:
                name = f"{self._thread_name_prefix}_{idx}"
            thread = threading.Thread(
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


class ProcessPoolExecutor(Executor):
    def __init__(
        self,
        max_workers: int | None = None,
        mp_context: Any | None = None,
        initializer: Callable[..., Any] | None = None,
        initargs: tuple[Any, ...] | list[Any] = (),
        *,
        max_tasks_per_child: int | None = None,
    ) -> None:
        if max_workers is None:
            max_workers = os.cpu_count() or 1
        if max_workers <= 0:
            raise ValueError("max_workers must be greater than 0")
        if initializer is not None and not callable(initializer):
            raise TypeError("initializer must be a callable")
        if max_tasks_per_child is not None:
            if not isinstance(max_tasks_per_child, int) or max_tasks_per_child <= 0:
                raise ValueError("max_tasks_per_child must be a positive int or None")

        self._max_workers = int(max_workers)
        self._pending_queue: deque[_ProcessTask] = deque()
        self._running_tasks: list[_ProcessTask] = []
        self._manager_lock = threading.Lock()
        self._manager_cond = threading.Condition(self._manager_lock)
        self._shutdown = False
        self._cancel_futures = False
        self._broken: BrokenProcessPool | None = None

        self._pool = _molt_multiprocessing.Pool(
            processes=self._max_workers,
            initializer=initializer,
            initargs=tuple(initargs),
            maxtasksperchild=max_tasks_per_child,
            context=mp_context,
        )

        self._manager = threading.Thread(
            target=_processpool_manager_worker,
            args=(self,),
            name="ProcessPoolExecutor-manager",
            daemon=False,
        )
        self._manager.start()

    def _submit_locked(
        self,
        fn: Callable[..., Any],
        args: tuple[Any, ...],
        kwargs: dict[str, Any],
    ) -> Future:
        if self._broken is not None:
            raise self._broken
        if self._shutdown:
            raise RuntimeError("cannot schedule new futures after shutdown")
        future = Future()
        self._pending_queue.append(
            _ProcessTask(fn=fn, args=args, kwargs=kwargs, future=future)
        )
        self._dispatch_locked()
        self._manager_cond.notify_all()
        return future

    def submit(self, fn: Callable[..., Any], /, *args: Any, **kwargs: Any) -> Future:
        if fn is None or not callable(fn):
            raise TypeError("submit expects a callable")
        with self._manager_cond:
            return self._submit_locked(fn, tuple(args), dict(kwargs))

    def map(
        self,
        fn: Callable[..., Any],
        *iterables: Iterable[Any],
        timeout: float | None = None,
        chunksize: int = 1,
    ) -> Iterator[Any]:
        if chunksize < 1:
            raise ValueError("chunksize must be >= 1")
        return super().map(
            fn,
            *iterables,
            timeout=timeout,
            chunksize=chunksize,
        )

    def _dispatch_locked(self) -> bool:
        progressed = False
        while self._pending_queue and len(self._running_tasks) < self._max_workers:
            task = self._pending_queue.popleft()
            if task.future.cancelled():
                progressed = True
                continue
            if self._shutdown and self._cancel_futures:
                task.future.cancel()
                progressed = True
                continue
            try:
                if not task.future.set_running_or_notify_cancel():
                    progressed = True
                    continue
            except InvalidStateError:
                progressed = True
                continue
            try:
                task.async_result = self._pool.apply_async(
                    task.fn, task.args, task.kwargs
                )
            except BaseException as exc:  # noqa: BLE001
                broken = BrokenProcessPool("ProcessPoolExecutor cannot submit work")
                broken.__cause__ = exc
                self._broken = broken
                self._shutdown = True
                try:
                    task.future.set_exception(broken)
                except InvalidStateError:
                    pass
                for queued in self._pending_queue:
                    if not queued.future.done():
                        try:
                            queued.future.set_exception(broken)
                        except InvalidStateError:
                            pass
                self._pending_queue.clear()
                self._manager_cond.notify_all()
                return True
            self._running_tasks.append(task)
            progressed = True
        return progressed

    def _complete_task(self, task: _ProcessTask) -> bool:
        async_result = task.async_result
        if async_result is None:
            return False

        try:
            result = async_result.get(timeout=0.0)
        except _BuiltinTimeoutError:
            return False
        except BaseException as exc:  # noqa: BLE001
            with self._manager_cond:
                try:
                    self._running_tasks.remove(task)
                except ValueError:
                    pass
                self._manager_cond.notify_all()
            if not task.future.done():
                try:
                    task.future.set_exception(exc)
                except InvalidStateError:
                    pass
        else:
            with self._manager_cond:
                try:
                    self._running_tasks.remove(task)
                except ValueError:
                    pass
                self._manager_cond.notify_all()
            if not task.future.done():
                try:
                    task.future.set_result(result)
                except InvalidStateError:
                    pass
        return True

    def _manager_loop(self) -> None:
        while True:
            progressed = False
            try:
                self._pool._poll_results()
            except BaseException as exc:  # noqa: BLE001
                broken = (
                    exc
                    if isinstance(exc, BrokenProcessPool)
                    else BrokenProcessPool("ProcessPoolExecutor worker failure")
                )
                if broken is not exc:
                    broken.__cause__ = exc
                with self._manager_cond:
                    self._broken = broken
                    self._shutdown = True
                    for task in self._running_tasks:
                        if not task.future.done():
                            try:
                                task.future.set_exception(broken)
                            except InvalidStateError:
                                pass
                    self._running_tasks.clear()
                    while self._pending_queue:
                        queued = self._pending_queue.popleft()
                        if not queued.future.done():
                            try:
                                queued.future.set_exception(broken)
                            except InvalidStateError:
                                pass
                    self._manager_cond.notify_all()
                break
            with self._manager_cond:
                if self._shutdown and self._cancel_futures:
                    while self._pending_queue:
                        pending = self._pending_queue.popleft()
                        pending.future.cancel()
                progressed = self._dispatch_locked() or progressed
                running_snapshot = list(self._running_tasks)
                should_exit = (
                    self._shutdown
                    and not self._pending_queue
                    and not self._running_tasks
                )
            if should_exit:
                break

            for task in running_snapshot:
                progressed = self._complete_task(task) or progressed

            if progressed:
                continue

            with self._manager_cond:
                if (
                    self._shutdown
                    and not self._pending_queue
                    and not self._running_tasks
                ):
                    break
                self._manager_cond.wait(0.002)

        try:
            self._pool.close()
            self._pool.join()
        except Exception:
            try:
                self._pool.terminate()
                self._pool.join()
            except Exception:
                pass

    def shutdown(self, wait: bool = True, *, cancel_futures: bool = False) -> None:
        with self._manager_cond:
            self._shutdown = True
            if cancel_futures:
                self._cancel_futures = True
                while self._pending_queue:
                    task = self._pending_queue.popleft()
                    task.future.cancel()
            self._manager_cond.notify_all()
        if wait:
            self._manager.join()


def _threadpool_worker_bootstrap(executor: ThreadPoolExecutor) -> None:
    executor._worker_bootstrap()


def _molt_threadpool_worker(executor: ThreadPoolExecutor, item: _WorkItem) -> None:
    try:
        if not item.future.set_running_or_notify_cancel():
            return
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


def _processpool_manager_worker(executor: ProcessPoolExecutor) -> None:
    executor._manager_loop()


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

    notifier = threading.Condition()

    def _notify(_: Future) -> None:
        with notifier:
            notifier.notify_all()

    for fut in pending:
        fut.add_done_callback(_notify)

    deadline = None if timeout is None else (time.monotonic() + float(timeout))
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
            remaining = deadline - time.monotonic()
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
    deadline = None if timeout is None else (time.monotonic() + float(timeout))

    while pending:
        remaining = None
        if deadline is not None:
            remaining = deadline - time.monotonic()
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
