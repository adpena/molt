"""Capability-gated asyncio shim for Molt."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Callable

import contextvars as _contextvars

from molt.concurrency import channel, current_token, set_current_token, spawn

__all__ = [
    "CancelledError",
    "Future",
    "Event",
    "InvalidStateError",
    "Queue",
    "Task",
    "TimeoutError",
    "create_task",
    "current_task",
    "ensure_future",
    "gather",
    "get_event_loop",
    "get_running_loop",
    "new_event_loop",
    "run",
    "set_event_loop",
    "sleep",
    "wait_for",
]

if TYPE_CHECKING:

    def molt_async_sleep(_delay: float = 0.0, _result: Any | None = None) -> Any:
        pass

    def molt_block_on(awaitable: Any) -> Any:
        pass


def _is_cancelled_exc(exc: BaseException) -> bool:
    return type(exc).__name__ == "CancelledError"


class CancelledError(BaseException):
    pass


class InvalidStateError(Exception):
    pass


class TimeoutError(Exception):
    pass


class Future:
    def __init__(self) -> None:
        self._done = False
        self._cancelled = False
        self._result: Any = None
        self._exception: BaseException | None = None
        self._molt_event_owner: Event | None = None
        self._molt_event_token_id: int | None = None
        self._callbacks: list[Callable[["Future"], Any]] = []

    def cancel(self) -> bool:
        if self._done:
            return False
        self._cancelled = True
        self._exception = None
        self._done = True
        self._invoke_callbacks()
        return True

    def cancelled(self) -> bool:
        return self._cancelled

    def done(self) -> bool:
        return self._done

    def result(self) -> Any:
        if not self._done:
            raise InvalidStateError("Result is not ready")
        if self._cancelled:
            if self._exception is not None:
                raise self._exception
            raise CancelledError
        if self._exception is not None:
            raise self._exception
        return self._result

    def exception(self) -> BaseException | None:
        if not self._done:
            raise InvalidStateError("Result is not ready")
        if self._cancelled:
            if self._exception is not None:
                raise self._exception
            raise CancelledError
        return self._exception

    def add_done_callback(self, fn: Callable[["Future"], Any]) -> None:
        if self._done:
            try:
                fn(self)
            except Exception:
                return None
            return None
        self._callbacks.append(fn)
        return None

    def set_result(self, result: Any) -> None:
        if self._done:
            raise InvalidStateError("Result is already set")
        self._result = result
        self._done = True
        self._invoke_callbacks()

    def set_exception(self, exception: BaseException) -> None:
        if self._done:
            raise InvalidStateError("Result is already set")
        self._exception = exception
        if _is_cancelled_exc(exception):
            self._cancelled = True
        self._done = True
        self._invoke_callbacks()

    def _invoke_callbacks(self) -> None:
        callbacks = self._callbacks
        self._callbacks = []
        idx = 0
        while idx < len(callbacks):
            fn = callbacks[idx]
            try:
                fn(self)
            except Exception:
                pass
            idx += 1

    async def _wait(self) -> Any:
        while not self._done:
            await molt_async_sleep(0.0)
        return self.result()

    def __await__(self) -> Any:
        return self._wait()


_TASKS: dict[int, "Task"] = {}
_EVENT_WAITERS: dict[int, list[Future]] = {}


def _register_event_waiter(token_id: int, fut: Future) -> None:
    waiters = _EVENT_WAITERS.get(token_id)
    if waiters is None:
        _EVENT_WAITERS[token_id] = [fut]
    else:
        waiters.append(fut)


def _unregister_event_waiter(token_id: int, fut: Future) -> None:
    waiters = _EVENT_WAITERS.get(token_id)
    if not waiters:
        return None
    idx = 0
    while idx < len(waiters):
        if waiters[idx] is fut:
            del waiters[idx]
            break
        idx += 1
    if not waiters:
        _EVENT_WAITERS.pop(token_id, None)


def _cleanup_event_waiters_for_token(token_id: int) -> None:
    waiters = _EVENT_WAITERS.pop(token_id, [])
    idx = 0
    while idx < len(waiters):
        fut = waiters[idx]
        event = getattr(fut, "_molt_event_owner", None)
        if event is not None:
            jdx = 0
            while jdx < len(event._waiters):
                if event._waiters[jdx] is fut:
                    del event._waiters[jdx]
                    break
                jdx += 1
        idx += 1


class Task(Future):
    def __init__(self, coro: Any) -> None:
        super().__init__()
        self._coro = coro
        parent = current_token()
        self._token = parent.child()
        ctx = _contextvars.copy_context()
        _contextvars._set_context_for_token(  # type: ignore[unresolved-attribute]
            self._token.token_id(),
            ctx,
        )
        _TASKS[self._token.token_id()] = self
        prev = set_current_token(self._token)
        try:
            spawn(self._runner())
        finally:
            set_current_token(prev)

    def cancel(self) -> bool:
        if self._done:
            return False
        self._token.cancel()
        return True

    def get_coro(self) -> Any:
        return self._coro

    async def _runner(self) -> None:
        result: Any = None
        exc: BaseException | None = None
        try:
            result = await self._coro
        except BaseException as err:
            exc = err
        if exc is None:
            self.set_result(result)
        else:
            self.set_exception(exc)
        _cleanup_event_waiters_for_token(self._token.token_id())
        _TASKS.pop(self._token.token_id(), None)
        _contextvars._clear_context_for_token(  # type: ignore[unresolved-attribute]
            self._token.token_id()
        )


class Event:
    def __init__(self) -> None:
        self._flag = False
        self._waiters: list[Future] = []

    def is_set(self) -> bool:
        return self._flag

    def set(self) -> None:
        if self._flag:
            return None
        self._flag = True
        waiters = self._waiters
        self._waiters = []
        idx = 0
        while idx < len(waiters):
            fut = waiters[idx]
            token_id = getattr(fut, "_molt_event_token_id", None)
            if isinstance(token_id, int):
                _unregister_event_waiter(token_id, fut)
            fut.set_result(True)
            idx += 1
        return None

    def clear(self) -> None:
        self._flag = False

    async def wait(self) -> bool:
        if self._flag:
            return True
        fut = Future()
        fut._molt_event_owner = self
        token_id = current_token().token_id()
        fut._molt_event_token_id = token_id
        self._waiters.append(fut)
        _register_event_waiter(token_id, fut)
        return await fut


class EventLoop:
    def create_task(self, coro: Any) -> Task:
        return Task(coro)

    def run_until_complete(self, awaitable: Any) -> Any:
        global _RUNNING_LOOP
        prev = _RUNNING_LOOP
        _RUNNING_LOOP = self
        result: Any = None
        try:
            result = molt_block_on(awaitable)
        finally:
            _RUNNING_LOOP = prev
        return result


_DEFAULT_LOOP = EventLoop()
_EVENT_LOOP: EventLoop | None = _DEFAULT_LOOP
_RUNNING_LOOP: EventLoop | None = None


def get_running_loop() -> EventLoop:
    if _RUNNING_LOOP is None:
        raise RuntimeError("no running event loop")
    return _RUNNING_LOOP


def get_event_loop() -> EventLoop:
    global _EVENT_LOOP
    if _EVENT_LOOP is None:
        _EVENT_LOOP = _DEFAULT_LOOP
    return _EVENT_LOOP


def set_event_loop(loop: EventLoop | None) -> None:
    global _EVENT_LOOP
    _EVENT_LOOP = loop


def new_event_loop() -> EventLoop:
    return _DEFAULT_LOOP


def run(awaitable: Any) -> Any:
    loop = new_event_loop()
    set_event_loop(loop)
    result: Any = None
    try:
        result = loop.run_until_complete(awaitable)
    finally:
        set_event_loop(None)
    return result


def sleep(delay: float = 0.0, result: Any | None = None) -> Any:
    if result is None:
        return molt_async_sleep(delay)
    return molt_async_sleep(delay, result)


def create_task(coro: Any) -> Task:
    get_running_loop()
    return Task(coro)


def ensure_future(awaitable: Any) -> Future:
    if isinstance(awaitable, Future):
        return awaitable
    return Task(awaitable)


def current_task() -> Task | None:
    token_id = current_token().token_id()
    return _TASKS.get(token_id)


async def wait_for(awaitable: Any, timeout: float | None) -> Any:
    if timeout is None:
        fut = ensure_future(awaitable)
        return await fut
    fut = ensure_future(awaitable)
    if fut.done():
        return fut.result()
    timeout_val = float(timeout)
    if timeout_val <= 0.0:
        fut.cancel()
        try:
            return await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                raise TimeoutError
            raise
    timer = ensure_future(sleep(timeout_val))
    try:
        while True:
            if fut.done():
                timer.cancel()
                return fut.result()
            if timer.done():
                fut.cancel()
                try:
                    return await fut
                except BaseException as exc:
                    if _is_cancelled_exc(exc):
                        raise TimeoutError
                    raise
            await sleep(0.0)
    except BaseException as exc:
        if _is_cancelled_exc(exc):
            fut.cancel()
            timer.cancel()
        raise


async def gather(*aws: Any, return_exceptions: bool = False) -> list[Any]:
    if not aws:
        return []
    tasks: list[Future] = []
    for aw in aws:
        tasks.append(ensure_future(aw))
    results: list[Any] = [None] * len(tasks)
    done = [False] * len(tasks)
    completed = 0
    while completed < len(tasks):
        progress = False
        idx = 0
        while idx < len(tasks):
            if not done[idx]:
                task = tasks[idx]
                if task.done():
                    done[idx] = True
                    completed += 1
                    progress = True
                    exc: BaseException | None = None
                    if task.cancelled():
                        exc = CancelledError()
                    else:
                        exc = task.exception()
                    if exc is not None:
                        if return_exceptions:
                            results[idx] = exc
                        else:
                            jdx = 0
                            while jdx < len(tasks):
                                if not done[jdx]:
                                    tasks[jdx].cancel()
                                jdx += 1
                            raise exc
                    else:
                        results[idx] = task.result()
            idx += 1
        if not progress:
            await sleep(0.0)
    return results


class Queue:
    def __init__(self, maxsize: int = 0) -> None:
        self._chan = channel(maxsize)

    async def put(self, item: Any) -> None:
        await self._chan.send_async(item)

    async def get(self) -> Any:
        return await self._chan.recv_async()
