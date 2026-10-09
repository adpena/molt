"""Task, runner, timeout, and wait/gather authority for ``asyncio.tasks``."""

from __future__ import annotations

import asyncio as _asyncio
import concurrent
import concurrent.futures
import contextvars
from dataclasses import dataclass
import functools
import inspect
import itertools
import sys as _sys
import signal as _signal
import threading as _threading
import time as _time
import types as _types
import warnings
import weakref
from typing import TYPE_CHECKING, Any, Callable, Iterable, Iterator

from _intrinsics import require_intrinsic as _require_intrinsic
_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from .futures import Future, _get_loop, _subscribe_completion, _unsubscribe_completion
from . import constants as _constants
from asyncio import (
    TimeoutError,
    _EXPOSE_GRAPH,
    _VERSION_INFO,
    _asyncio_future_transfer,
    _asyncio_tasks_add_done_callback,
    _is_cancelled_exc,
    _require_asyncio_intrinsic,
    _task_registry_contains,
    _task_registry_current_for_loop,
    _task_registry_pop,
    _task_registry_set,
    _event_waiters_register,
    _event_waiters_unregister,
    _event_waiters_cleanup_token,
    iscoroutine,
    _molt_async_sleep,
    _molt_asyncio_future_cancelled,
    _molt_asyncio_future_done,
    _molt_asyncio_task_cancel_apply,
    _molt_asyncio_task_last_exception_clear,
    _molt_asyncio_task_registry_live_set,
    _molt_asyncio_task_uncancel_apply,
    _molt_cancel_token_cancel,
    _molt_cancel_token_clone,
    _molt_cancel_token_drop,
    _molt_cancel_token_get_current,
    _molt_cancel_token_is_cancelled,
    _molt_cancel_token_new,
    _molt_cancel_token_set_current,
    _molt_spawn,
    _molt_task_register_token_owned,
)

if TYPE_CHECKING:
    from asyncio import EventLoop, Handle, Queue, TimerHandle

GenericAlias = _types.GenericAlias
_contextvars = contextvars
types = _types
base_tasks: Any | None = None
coroutines: Any | None = None
events: Any | None = None
exceptions: Any | None = None
futures: Any | None = None
timeouts: Any | None = None

def _get_running_loop() -> Any:
    return _asyncio._get_running_loop()

def get_running_loop() -> Any:
    return _asyncio.get_running_loop()

def get_event_loop() -> Any:
    return _asyncio.get_event_loop()

def new_event_loop() -> Any:
    return _asyncio.new_event_loop()

def set_event_loop(loop: Any | None) -> None:
    return _asyncio.set_event_loop(loop)

def _cancel_all_tasks(loop: Any) -> None:
    return _asyncio._cancel_all_tasks(loop)

def _queue_type() -> type[Any]:
    return _asyncio.Queue

FIRST_COMPLETED = object()
FIRST_EXCEPTION = object()
ALL_COMPLETED = object()

def spawn(task: Any) -> None:
    _molt_spawn(task)

class CancellationToken:
    def __init__(self) -> None:
        self._token = int(_molt_cancel_token_new(None))
        self._owned = True

    @classmethod
    def detached(cls) -> "CancellationToken":
        token = cls()
        old_id = token._token
        token._token = int(_molt_cancel_token_new(-1))
        _molt_cancel_token_drop(old_id)
        return token

    def child(self) -> "CancellationToken":
        token = CancellationToken()
        old_id = token._token
        token._token = int(_molt_cancel_token_new(self._token))
        _molt_cancel_token_drop(old_id)
        return token

    def cancelled(self) -> bool:
        return bool(_molt_cancel_token_is_cancelled(self._token))

    def cancel(self) -> None:
        _molt_cancel_token_cancel(self._token)

    def set_current(self) -> "CancellationToken":
        prev_id = int(_molt_cancel_token_set_current(self._token))
        return _wrap_existing_token(prev_id, False)

    def token_id(self) -> int:
        return int(self._token)

    def __del__(self) -> None:
        if getattr(self, "_owned", False):
            _molt_cancel_token_drop(int(self._token))

def _wrap_existing_token(token_id: int, owned: bool) -> CancellationToken:
    token = CancellationToken()
    old_id = token._token
    token._token = int(token_id)
    token._owned = bool(owned)
    if owned:
        _molt_cancel_token_clone(int(token_id))
    if old_id != token_id:
        _molt_cancel_token_drop(int(old_id))
    return token

def _swap_current_token(token: CancellationToken) -> int:
    if _molt_cancel_token_set_current is not None:  # type: ignore[name-defined]
        return _molt_cancel_token_set_current(token.token_id())  # type: ignore[name-defined]
    return 0

def _restore_token_id(token_id: int) -> None:
    if _molt_cancel_token_set_current is not None:  # type: ignore[name-defined]
        _molt_cancel_token_set_current(token_id)  # type: ignore[name-defined]
    return None

def _current_token_id() -> int:
    if _molt_cancel_token_get_current is not None:  # type: ignore[name-defined]
        return _molt_cancel_token_get_current()  # type: ignore[name-defined]
    return 0

def _future_done(task: Any) -> bool:
    if isinstance(task, Future):
        return bool(_molt_asyncio_future_done(task._fut_handle))
    done_fn = getattr(task, "done", None)
    if callable(done_fn):
        return done_fn()
    return False

def _future_cancelled(task: Any) -> bool:
    if isinstance(task, Future):
        return bool(_molt_asyncio_future_cancelled(task._fut_handle))
    cancelled_fn = getattr(task, "cancelled", None)
    if callable(cancelled_fn):
        return cancelled_fn()
    return False

def _future_exception(task: Any) -> BaseException | None:
    if isinstance(task, Future):
        return task._exception
    try:
        return task.exception()
    except BaseException as err:
        return err

def _register_event_waiter(token_id: int, fut: Future) -> None:
    _event_waiters_register(token_id, fut)

def _unregister_event_waiter(token_id: int, fut: Future) -> None:
    _event_waiters_unregister(token_id, fut)

def _cleanup_event_waiters_for_token(token_id: int) -> None:
    _event_waiters_cleanup_token(token_id)

_TASK_COUNTER = 0

def _next_task_name() -> str:
    global _TASK_COUNTER
    _TASK_COUNTER += 1
    return f"Task-{_TASK_COUNTER}"

class Task(Future):
    _coro: Any
    _runner_task: Any | None
    _token: CancellationToken
    _loop: "EventLoop | None"
    _name: str
    _cancel_requested: int
    _cancel_message: Any | None
    _context: Any | None
    _fut_waiter: Future | None

    def __init__(
        self,
        coro: Any,
        *,
        loop: "EventLoop | None" = None,
        name: str | None = None,
        context: Any | None = None,
    ) -> None:
        super().__init__(loop=loop)
        if not iscoroutine(coro):
            raise TypeError("a coroutine was expected, got {!r}".format(coro))
        self._coro = coro
        task_dict = getattr(self, "__dict__", None)
        if isinstance(task_dict, dict):
            task_dict["_coro"] = coro
        self._runner_task: Any | None = None
        self._token = CancellationToken.detached()
        if loop is not None:
            self._loop = loop
        self._name = _next_task_name() if name is None else str(name)
        self._cancel_requested = 0
        self._cancel_message: Any | None = None
        if context is None:
            context = _contextvars.copy_context()
        self._context = context
        _contextvars._set_context_for_token(  # type: ignore[unresolved-attribute]
            self._token.token_id(),
            context,
        )
        _task_registry_set(self._token.token_id(), self)
        self._fut_waiter = None
        token_id = self._token.token_id()
        if _molt_task_register_token_owned is not None:  # type: ignore[name-defined]
            _molt_task_register_token_owned(self._coro, token_id)  # type: ignore[name-defined]
        prev_id = _swap_current_token(self._token)
        try:
            runner = self._runner(self._coro)
            self._runner_task = runner
            if _molt_task_register_token_owned is not None:  # type: ignore[name-defined]
                _molt_task_register_token_owned(  # type: ignore[name-defined]
                    runner, token_id
                )
            self._loop._spawn_task(runner)
        except BaseException:
            self._runner_task = None
            _task_registry_pop(token_id)
            _contextvars._clear_context_for_token(token_id)
            raise
        finally:
            _restore_token_id(prev_id)


    def cancel(self, msg: Any | None = None) -> bool:
        if self.done():
            return False
        self._cancel_requested += 1
        waiter = self._fut_waiter
        if waiter is not None and waiter.cancel(msg=msg):
            return True
        self._cancel_message = msg
        _require_asyncio_intrinsic(
            _molt_asyncio_task_cancel_apply, "asyncio_task_cancel_apply"
        )(self._coro, msg)
        return True

    def get_coro(self) -> Any:
        try:
            return self._coro
        except AttributeError:
            task_dict = getattr(self, "__dict__", None)
            if isinstance(task_dict, dict) and "_coro" in task_dict:
                return task_dict["_coro"]
            raise

    def get_name(self) -> str:
        return self._name

    def set_name(self, value: str) -> None:
        self._name = str(value)

    def get_context(self) -> Any:
        return self._context

    def cancelling(self) -> int:
        return self._cancel_requested

    def uncancel(self) -> int:
        if self._cancel_requested <= 0:
            return 0
        self._cancel_requested -= 1
        if self._cancel_requested == 0 and _VERSION_INFO >= (3, 13):
            self._cancel_message = None
            _require_asyncio_intrinsic(
                _molt_asyncio_task_uncancel_apply, "asyncio_task_uncancel_apply"
            )(self._coro)
        return self._cancel_requested

    async def _runner(self, coro: Any | None = None) -> None:
        result: Any = None
        exc: BaseException | None = None
        extra_token_id: int | None = None
        if coro is None:
            coro = getattr(self, "_coro")
        current_id = _current_token_id()
        if current_id != self._token.token_id() and not _task_registry_contains(
            current_id
        ):
            _task_registry_set(current_id, self)
            extra_token_id = current_id
        try:
            result = await coro
        except BaseException as err:
            exc = err
        if exc is None:
            if not _molt_asyncio_future_done(self._fut_handle):
                Future.set_result(self, result)
            _molt_asyncio_task_last_exception_clear(coro)
        else:
            if not _molt_asyncio_future_done(self._fut_handle):
                if _is_cancelled_exc(exc):
                    self._set_cancelled(exc, self._cancel_message)
                else:
                    Future.set_exception(self, exc)
        self._fut_waiter = None
        _cleanup_event_waiters_for_token(self._token.token_id())
        _task_registry_pop(self._token.token_id())
        if extra_token_id is not None:
            _task_registry_pop(extra_token_id)
        _contextvars._clear_context_for_token(  # type: ignore[unresolved-attribute]
            self._token.token_id()
        )
        self._runner_task = None

        if isinstance(exc, (KeyboardInterrupt, SystemExit)):
            raise exc

    def set_result(self, result: Any) -> None:
        raise RuntimeError("Task does not support set_result operation")

    def set_exception(self, exception: BaseException) -> None:
        raise RuntimeError("Task does not support set_exception operation")

    def __repr__(self) -> str:
        if _molt_asyncio_future_cancelled(self._fut_handle):
            state = "cancelled"
        elif _molt_asyncio_future_done(self._fut_handle):
            state = "finished"
        else:
            state = "pending"
        return f"<Task {self._name} {state}>"

class TaskGroup:
    def __init__(self) -> None:
        self._entered = False
        self._exiting = False
        self._aborting = False
        self._loop: Any = None
        self._parent_task: Any = None
        self._parent_cancel_requested = False
        self._tasks: set[Task] = set()
        self._errors: list[BaseException] = []
        self._base_error: BaseException | None = None
        self._on_completed_fut: Future | None = None

    async def __aenter__(self) -> "TaskGroup":
        if self._entered:
            raise RuntimeError("TaskGroup has already been entered")
        self._loop = get_running_loop()
        self._parent_task = current_task(self._loop)
        if self._parent_task is None:
            raise RuntimeError("TaskGroup cannot determine the parent task")
        self._entered = True
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        try:
            return await self._aexit(exc_type, exc)
        finally:
            self._parent_task = None
            self._errors = []
            self._base_error = None
            self._on_completed_fut = None

    async def _aexit(self, exc_type: Any, exc: Any) -> bool:
        self._exiting = True
        if isinstance(exc, (KeyboardInterrupt, SystemExit)):
            if self._base_error is None:
                self._base_error = exc
        cancellation = exc if _is_cancelled_exc(exc) else None
        # 3.13 moved this balancing operation after the children finish.
        if _VERSION_INFO < (3, 13) and self._parent_cancel_requested:
            if self._parent_task.uncancel() == 0:
                cancellation = None
        if exc_type is not None and not self._aborting:
            self._abort()
        while self._tasks:
            self._on_completed_fut = self._loop.create_future()
            try:
                await self._on_completed_fut
            except _asyncio.CancelledError as err:
                if not self._aborting:
                    cancellation = err
                    self._abort()
            finally:
                self._on_completed_fut = None
        if self._base_error is not None:
            raise self._base_error
        if _VERSION_INFO >= (3, 13) and self._parent_cancel_requested:
            if self._parent_task.uncancel() == 0:
                cancellation = None
        if cancellation is not None and not self._errors:
            raise cancellation
        if exc_type is not None and not _is_cancelled_exc(exc):
            self._errors.append(exc)
        if self._errors:
            if _VERSION_INFO >= (3, 13) and self._parent_task.cancelling():
                self._parent_task.uncancel()
                self._parent_task.cancel()
            raise BaseExceptionGroup("unhandled errors in a TaskGroup", self._errors) from None
        return False

    def create_task(
        self, coro: Any, *, name: str | None = None, context: Any | None = None
    ) -> Task:
        error = None
        if not self._entered:
            error = "TaskGroup has not been entered"
        elif self._exiting and not self._tasks:
            error = "TaskGroup is finished"
        elif self._aborting:
            error = "TaskGroup is shutting down"
        if error is not None:
            if _VERSION_INFO >= (3, 13):
                coro.close()
            raise RuntimeError(error)
        task = self._loop.create_task(coro, name=name, context=context)
        self._tasks.add(task)
        task.add_done_callback(self._on_task_done)
        return task

    def _abort(self) -> None:
        self._aborting = True
        for task in self._tasks:
            if not task.done():
                task.cancel()

    def _on_task_done(self, task: Future) -> None:
        self._tasks.discard(task)
        waiter = self._on_completed_fut
        if waiter is not None and not self._tasks and not waiter.done():
            waiter.set_result(None)
        if task.cancelled():
            return
        exc = task.exception()
        if exc is None:
            return
        self._errors.append(exc)
        if isinstance(exc, (KeyboardInterrupt, SystemExit)) and self._base_error is None:
            self._base_error = exc
        if self._parent_task.done():
            self._loop.call_exception_handler({
                "message": "Task has errored out but its parent task is already completed",
                "exception": exc,
                "task": task,
            })
        elif not self._aborting and not self._parent_cancel_requested:
            self._abort()
            self._parent_cancel_requested = True
            self._parent_task.cancel()


class _Timeout:
    def __init__(self, when: float | None) -> None:
        self._when = when
        self._state = "created"
        self._task: Task | None = None
        self._handle: Any = None
        self._cancelling = 0

    def when(self) -> float | None:
        return self._when

    def reschedule(self, when: float | None) -> None:
        if self._state != "active":
            if self._state == "created":
                raise RuntimeError("Timeout has not been entered")
            raise RuntimeError(f"Cannot change state of {self._state} Timeout")
        self._when = when
        if self._handle is not None:
            self._handle.cancel()
            self._handle = None
        if when is not None:
            loop = get_running_loop()
            if when <= loop.time():
                self._handle = loop.call_soon(self._on_timeout)
            else:
                self._handle = loop.call_at(when, self._on_timeout)

    def expired(self) -> bool:
        return self._state in ("expiring", "expired")

    def _on_timeout(self) -> None:
        self._task.cancel()
        self._state = "expiring"
        self._handle = None

    async def __aenter__(self) -> "_Timeout":
        if self._state != "created":
            raise RuntimeError("Timeout has already been entered")
        task = current_task()
        if task is None:
            raise RuntimeError("Timeout should be used inside a task")
        self._task = task
        self._cancelling = task.cancelling()
        self._state = "active"
        self.reschedule(self._when)
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        if self._handle is not None:
            self._handle.cancel()
            self._handle = None
        if self._state == "expiring":
            self._state = "expired"
            if self._task.uncancel() <= self._cancelling and exc is not None:
                if _is_cancelled_exc(exc):
                    raise TimeoutError from exc
                if _VERSION_INFO >= (3, 13):
                    self._insert_timeout_error(exc)
                    if isinstance(exc, ExceptionGroup):
                        for child in exc.exceptions:
                            self._insert_timeout_error(child)
        elif self._state == "active":
            self._state = "finished"
        return False

    @staticmethod
    def _insert_timeout_error(exc: BaseException) -> None:
        while exc.__context__ is not None:
            if _is_cancelled_exc(exc.__context__):
                timeout_error = TimeoutError()
                timeout_error.__cause__ = exc.__context__
                timeout_error.__context__ = exc.__context__
                exc.__context__ = timeout_error
                return
            exc = exc.__context__

class Runner:
    def __init__(
        self,
        *,
        debug: bool | None = None,
        loop_factory: Callable[[], "EventLoop"] | None = None,
    ) -> None:
        self._state = "created"
        self._loop: EventLoop | None = None
        self._debug = debug
        self._loop_factory = loop_factory
        self._context: Any | None = None
        self._set_event_loop = False
        self._interrupt_count = 0

    def __enter__(self) -> "Runner":
        self._lazy_init()
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    def _lazy_init(self) -> None:
        if self._state == "closed":
            raise RuntimeError("Runner is closed")
        if self._state == "initialized":
            return
        if self._loop_factory is None:
            self._loop = new_event_loop()
            if not self._set_event_loop:
                set_event_loop(self._loop)
                self._set_event_loop = True
        else:
            self._loop = self._loop_factory()
        if self._debug is not None:
            self._loop.set_debug(self._debug)
        self._context = _contextvars.copy_context()
        self._state = "initialized"

    def get_loop(self) -> EventLoop:
        self._lazy_init()
        return self._loop

    def run(self, coro: Any, *, context: Any | None = None) -> Any:
        if _VERSION_INFO < (3, 14) and not iscoroutine(coro):
            raise ValueError("a coroutine was expected, got {!r}".format(coro))
        if _get_running_loop() is not None:
            raise RuntimeError(
                "Runner.run() cannot be called from a running event loop"
            )
        self._lazy_init()
        if _VERSION_INFO >= (3, 14) and not iscoroutine(coro):
            if not inspect.isawaitable(coro):
                raise TypeError("An asyncio.Future, a coroutine or an awaitable is required")

            async def await_result(awaitable: Any) -> Any:
                return await awaitable

            coro = await_result(coro)
        loop = self._loop
        if context is None:
            context = self._context
        task = loop.create_task(coro, context=context)

        sigint_handler = None
        if (
            _sys.platform not in ("emscripten", "wasi")
            and _MOLT_CAPABILITIES_HAS("signal.signal")
            and _threading.current_thread() is _threading.main_thread()
            and _signal.getsignal(_signal.SIGINT) is _signal.default_int_handler
        ):
            sigint_handler = functools.partial(self._on_sigint, main_task=task)
            try:
                _signal.signal(_signal.SIGINT, sigint_handler)
            except ValueError:
                sigint_handler = None
        self._interrupt_count = 0
        try:
            return loop.run_until_complete(task)
        except _asyncio.CancelledError:
            if self._interrupt_count > 0:
                uncancel = getattr(task, "uncancel", None)
                if uncancel is not None and uncancel() == 0:
                    raise KeyboardInterrupt()
            raise
        finally:
            if (
                sigint_handler is not None
                and _signal.getsignal(_signal.SIGINT) is sigint_handler
            ):
                _signal.signal(_signal.SIGINT, _signal.default_int_handler)

    def _on_sigint(self, signum: Any, frame: Any, main_task: Any) -> None:
        self._interrupt_count += 1
        if self._interrupt_count == 1 and not main_task.done():
            main_task.cancel()
            self._loop.call_soon_threadsafe(lambda: None)
            return
        raise KeyboardInterrupt()

    def close(self) -> None:
        if self._state != "initialized":
            return
        loop = self._loop
        try:
            _cancel_all_tasks(loop)
            loop.run_until_complete(loop.shutdown_asyncgens())
            loop.run_until_complete(
                loop.shutdown_default_executor(_constants.THREAD_JOIN_TIMEOUT)
            )
        finally:
            if self._set_event_loop:
                set_event_loop(None)
            loop.close()
            self._loop = None
            self._state = "closed"


def run(
    awaitable: Any,
    *,
    debug: bool | None = None,
    loop_factory: Callable[[], "EventLoop"] | None = None,
) -> Any:
    if _get_running_loop() is not None:
        raise RuntimeError("asyncio.run() cannot be called from a running event loop")
    with Runner(debug=debug, loop_factory=loop_factory) as runner:
        return runner.run(awaitable)

async def sleep(delay: float = 0.0, result: Any | None = None) -> Any:
    if delay <= 0:
        delay = 0.0
    else:
        delay = float(delay)
    fut = _require_asyncio_intrinsic(_molt_async_sleep, "async_sleep")(delay, result)
    return await fut

async def to_thread(func: Any, /, *args: Any, **kwargs: Any) -> Any:
    loop = get_running_loop()
    context = _contextvars.copy_context()
    call = functools.partial(func, *args, **kwargs)
    return await loop.run_in_executor(None, context.run, call)

def shield(awaitable: Any) -> Future:
    inner = ensure_future(awaitable)
    if inner.done():
        return inner
    outer = _get_loop(inner).create_future()

    def inner_done(done: Future) -> None:
        if outer.cancelled():
            if not done.cancelled():
                done.exception()
            return
        if done.cancelled():
            outer.cancel()
        else:
            exc = done.exception()
            if exc is not None:
                outer.set_exception(exc)
            else:
                outer.set_result(done.result())

    def outer_done(done: Future) -> None:
        if not inner.done():
            _unsubscribe_completion(inner, subscription)

    subscription = _subscribe_completion(inner, inner_done)
    outer.add_done_callback(outer_done)
    return outer

def eager_task_factory(
    loop: EventLoop,
    coro: Any,
    *,
    name: str | None = None,
    context: Any | None = None,
) -> Task:
    """Task factory that eagerly starts coroutine execution.

    Molt's scheduler already runs the coroutine until its first suspension
    point during task creation, so this is semantically equivalent to the
    CPython eager_start=True behaviour.
    """
    return Task(coro, loop=loop, name=name, context=context)

def create_eager_task_factory(
    custom_task_constructor: Callable[..., Task] | None = None,
) -> Callable[[EventLoop, Any], Task]:
    """Create a task factory for eager task execution.

    If *custom_task_constructor* is not ``None``, it must be a callable with
    the signature ``(coro, *, loop, name, context, eager_start)`` and is used
    instead of the default :class:`Task` constructor.
    """
    if custom_task_constructor is None:
        return eager_task_factory

    def _factory(
        loop: EventLoop,
        coro: Any,
        *,
        name: str | None = None,
        context: Any | None = None,
    ) -> Task:
        return custom_task_constructor(
            coro, loop=loop, name=name, context=context, eager_start=True
        )

    return _factory

def create_task(
    coro: Any, *, name: str | None = None, context: Any | None = None
) -> Task:
    loop = get_running_loop()
    return loop.create_task(coro, name=name, context=context)

def ensure_future(awaitable: Any, *, loop: EventLoop | None = None) -> Future:
    if _asyncio.isfuture(awaitable):
        if loop is not None and loop is not _get_loop(awaitable):
            raise ValueError("The future belongs to a different loop than the one specified as the loop argument")
        return awaitable
    should_close = True
    if not iscoroutine(awaitable):
        if not inspect.isawaitable(awaitable):
            raise TypeError("An asyncio.Future, a coroutine or an awaitable is required")

        async def await_result(value: Any) -> Any:
            return await value

        awaitable = await_result(awaitable)
        should_close = False
    if loop is None:
        loop = get_event_loop()
    try:
        return loop.create_task(awaitable)
    except RuntimeError:
        if should_close:
            awaitable.close()
        raise

def run_coroutine_threadsafe(coro: Any, loop: EventLoop) -> concurrent.futures.Future:
    if not iscoroutine(coro):
        raise TypeError("A coroutine object is required")
    fut = concurrent.futures.Future()

    def _schedule() -> None:
        try:
            task = loop.create_task(coro)
        except (KeyboardInterrupt, SystemExit):
            raise
        except BaseException as exc:
            if fut.set_running_or_notify_cancel():
                fut.set_exception(exc)
            raise

        def cancel_task(done: concurrent.futures.Future) -> None:
            if done.cancelled() and not loop.is_closed():
                loop.call_soon_threadsafe(task.cancel)

        def _transfer(done: Future) -> None:
            if done.cancelled():
                fut.cancel()
            elif fut.set_running_or_notify_cancel():
                exception = done.exception()
                if exception is not None:
                    fut.set_exception(exception)
                else:
                    fut.set_result(done.result())

        fut.add_done_callback(cancel_task)
        task.add_done_callback(_transfer)

    loop.call_soon_threadsafe(_schedule)
    return fut

def wrap_future(fut: Any, *, loop: EventLoop | None = None) -> Future:
    if _asyncio.isfuture(fut):
        return fut
    if not isinstance(fut, concurrent.futures.Future):
        raise TypeError("concurrent.futures.Future is required")
    if loop is None:
        loop = get_event_loop()
    proxy = Future(loop=loop)

    def transfer(done: Any) -> None:
        if proxy.done():
            return
        if done.cancelled():
            proxy.cancel()
            return
        exc = done.exception()
        if exc is not None:
            proxy.set_exception(exc)
        else:
            proxy.set_result(done.result())

    def schedule_transfer(done: Any) -> None:
        if not loop.is_closed():
            loop.call_soon_threadsafe(transfer, done)

    def cancel_source(done: Future) -> None:
        if done.cancelled():
            fut.cancel()

    proxy.add_done_callback(cancel_source)
    fut.add_done_callback(schedule_transfer)
    return proxy

def current_task(loop: EventLoop | None = None) -> Task | None:
    if loop is None:
        loop = get_running_loop()
    task = _task_registry_current_for_loop(loop)
    if task is None:
        return None
    return task if isinstance(task, Task) else None

def all_tasks(loop: EventLoop | None = None) -> set[Task]:
    if loop is None:
        loop = get_running_loop()
    task_values = _require_asyncio_intrinsic(
        _molt_asyncio_task_registry_live_set, "asyncio_task_registry_live_set"
    )(loop)
    if isinstance(task_values, set):
        return task_values
    if task_values is not None:
        return set(task_values)
    return set()

@dataclass(frozen=True, slots=True)
class FrameCallGraphEntry:
    frame: _types.FrameType

@dataclass(frozen=True, slots=True)
class FutureCallGraph:
    future: Future
    call_stack: tuple[FrameCallGraphEntry, ...]
    awaited_by: tuple["FutureCallGraph", ...]

def _build_graph_for_future(
    future: Future,
    *,
    limit: int | None = None,
) -> FutureCallGraph:
    if not isinstance(future, Future):
        raise TypeError(
            f"{future!r} object does not appear to be compatible with asyncio.Future"
        )
    coro = None
    get_coro = getattr(future, "get_coro", None)
    if get_coro is not None and limit != 0:
        coro = get_coro()
    stack: list[FrameCallGraphEntry] = []
    awaited_by: list[FutureCallGraph] = []
    while coro is not None:
        if hasattr(coro, "cr_await"):
            stack.append(FrameCallGraphEntry(coro.cr_frame))
            coro = coro.cr_await
        elif hasattr(coro, "ag_await"):
            stack.append(FrameCallGraphEntry(coro.ag_frame))
            coro = coro.ag_await
        else:
            break
    if future._asyncio_awaited_by:
        for parent in future._asyncio_awaited_by:
            awaited_by.append(_build_graph_for_future(parent, limit=limit))
    if limit is not None:
        if limit > 0:
            stack = stack[:limit]
        elif limit < 0:
            stack = stack[limit:]
    stack.reverse()
    return FutureCallGraph(future, tuple(stack), tuple(awaited_by))

def capture_call_graph(
    future: Future | None = None,
    /,
    *,
    depth: int = 1,
    limit: int | None = None,
) -> FutureCallGraph | None:
    loop = _get_running_loop()
    if future is not None:
        if loop is None or future is not current_task(loop=loop):
            return _build_graph_for_future(future, limit=limit)
    else:
        if loop is None:
            raise RuntimeError(
                "capture_call_graph() is called outside of a running event loop "
                "and no *future* to introspect was provided"
            )
        future = current_task(loop=loop)
    if future is None:
        return None
    if not isinstance(future, Future):
        raise TypeError(
            f"{future!r} object does not appear to be compatible with asyncio.Future"
        )
    call_stack: list[FrameCallGraphEntry] = []
    if limit == 0:
        frame = None
    else:
        frame = getattr(_sys, "_getframe", lambda _d: None)(depth)
    try:
        while frame is not None:
            gen = getattr(frame, "f_generator", None)
            is_async = gen is not None
            call_stack.append(FrameCallGraphEntry(frame))
            if is_async:
                back = frame.f_back
                if back is not None and getattr(back, "f_generator", None) is None:
                    break
            frame = frame.f_back
    finally:
        frame = None
    awaited_by = []
    if future._asyncio_awaited_by:
        for parent in future._asyncio_awaited_by:
            awaited_by.append(_build_graph_for_future(parent, limit=limit))
    if limit is not None:
        trim = limit * -1
        if trim > 0:
            call_stack = call_stack[:trim]
        elif trim < 0:
            call_stack = call_stack[trim:]
    return FutureCallGraph(future, tuple(call_stack), tuple(awaited_by))

def format_call_graph(
    future: Future | None = None,
    /,
    *,
    depth: int = 1,
    limit: int | None = None,
) -> str:
    def render_level(st: FutureCallGraph, buf: list[str], level: int) -> None:
        def add_line(line: str) -> None:
            buf.append(level * "    " + line)

        if isinstance(st.future, Task):
            add_line(f"* Task(name={st.future.get_name()!r}, id={id(st.future):#x})")
        else:
            add_line(f"* Future(id={id(st.future):#x})")
        if st.call_stack:
            add_line("  + Call stack:")
            for ste in st.call_stack:
                frame = ste.frame
                gen = getattr(frame, "f_generator", None)
                if gen is None:
                    add_line(
                        f"  |   File {frame.f_code.co_filename!r},"
                        f" line {frame.f_lineno}, in"
                        f" {frame.f_code.co_qualname}()"
                    )
                else:
                    try:
                        frame = gen.cr_frame
                        code = gen.cr_code
                        tag = "async"
                    except AttributeError:
                        try:
                            frame = gen.ag_frame
                            code = gen.ag_code
                            tag = "async generator"
                        except AttributeError:
                            frame = gen.gi_frame
                            code = gen.gi_code
                            tag = "generator"
                    add_line(
                        f"  |   File {frame.f_code.co_filename!r},"
                        f" line {frame.f_lineno}, in"
                        f" {tag} {code.co_qualname}()"
                    )
        if st.awaited_by:
            add_line("  + Awaited by:")
            for fut in st.awaited_by:
                render_level(fut, buf, level + 1)

    graph = capture_call_graph(future, depth=depth + 1, limit=limit)
    if graph is None:
        return ""
    buf: list[str] = []
    try:
        render_level(graph, buf, 0)
    finally:
        graph = None
    return "\n".join(buf)

def print_call_graph(
    future: Future | None = None,
    /,
    *,
    file: Any | None = None,
    depth: int = 1,
    limit: int | None = None,
) -> None:
    print(format_call_graph(future, depth=depth, limit=limit), file=file)


def _release_waiter(waiter: Future, *args: Any) -> None:
    if not waiter.done():
        waiter.set_result(None)


async def wait(
    aws: Any,
    timeout: float | None = None,
    return_when: object = ALL_COMPLETED,
) -> tuple[set[Future], set[Future]]:
    if _asyncio.isfuture(aws) or iscoroutine(aws):
        raise TypeError("expect a list of futures, not a single future or coroutine")
    if return_when not in (ALL_COMPLETED, FIRST_COMPLETED, FIRST_EXCEPTION):
        raise ValueError("Invalid return_when value")
    tasks = set(aws)
    if not tasks:
        raise ValueError("Set of Tasks/Futures is empty.")
    for task in tasks:
        if iscoroutine(task):
            raise TypeError("Passing coroutines is forbidden, use tasks explicitly.")
    loop = get_running_loop()
    waiter = loop.create_future()
    timer = None
    remaining = len(tasks)

    def done(task: Future) -> None:
        nonlocal remaining
        remaining -= 1
        if (
            remaining == 0
            or return_when is FIRST_COMPLETED
            or (return_when is FIRST_EXCEPTION and not task.cancelled() and task.exception() is not None)
        ):
            if timer is not None:
                timer.cancel()
            _release_waiter(waiter)

    if timeout is not None:
        timer = loop.call_later(timeout, _release_waiter, waiter)
    subscriptions = [(task, _subscribe_completion(task, done)) for task in tasks]
    try:
        await waiter
    finally:
        if timer is not None:
            timer.cancel()
        for task, subscription in subscriptions:
            _unsubscribe_completion(task, subscription)
    return ({task for task in tasks if task.done()}, {task for task in tasks if not task.done()})


async def _cancel_and_wait(fut: Future) -> None:
    waiter = get_running_loop().create_future()
    callback = functools.partial(_release_waiter, waiter)
    subscription = _subscribe_completion(fut, callback)
    try:
        fut.cancel()
        await waiter
    finally:
        _unsubscribe_completion(fut, subscription)


async def wait_for(awaitable: Any, timeout: float | None) -> Any:
    if timeout is not None and timeout <= 0:
        fut = ensure_future(awaitable)
        if fut.done():
            return fut.result()
        await _cancel_and_wait(fut)
        try:
            return fut.result()
        except _asyncio.CancelledError as exc:
            raise TimeoutError from exc
    async with _Timeout(None if timeout is None else get_running_loop().time() + timeout):
        return await awaitable

def timeout(delay: float | None) -> _Timeout:
    if delay is None:
        return _Timeout(None)
    loop = get_running_loop()
    return _Timeout(loop.time() + float(delay))

def timeout_at(when: float) -> _Timeout:
    return _Timeout(float(when))

def _cancelled_error(fut: Future) -> BaseException:
    try:
        fut.result()
    except _asyncio.CancelledError as exc:
        return exc
    return _asyncio.CancelledError()


class _GatheringFuture(Future):
    def __init__(self, children: list[Future], remaining: int, return_exceptions: bool, loop: Any) -> None:
        super().__init__(loop=loop)
        self._children = children
        self._remaining = remaining
        self._results: list[Any] = [None] * len(children)
        self._return_exceptions = return_exceptions
        self._cancel_requested = False

    def cancel(self, msg: Any = None) -> bool:
        if self.done():
            return False
        accepted = False
        for child in self._children:
            if child.cancel(msg=msg):
                accepted = True
        if accepted:
            self._cancel_requested = True
            self._cancel_message = msg
        return accepted

    def _finish_exception(self, exc: BaseException) -> None:
        self._children = []
        self._results = []
        self.set_exception(exc)

    def _child_done(self, positions: list[int], child: Future) -> None:
        self._remaining -= 1
        if self.done():
            if not child.cancelled():
                child.exception()
            return
        if child.cancelled():
            if not self._return_exceptions:
                self._finish_exception(_cancelled_error(child))
                return
            result: Any = _asyncio.CancelledError(
                "" if child._cancel_message is None else child._cancel_message
            )
        else:
            exc = child.exception()
            if exc is not None and not self._return_exceptions:
                self._finish_exception(exc)
                return
            result = child.result() if exc is None else exc
        for position in positions:
            self._results[position] = result
        if self._remaining == 0:
            if self._cancel_requested:
                if self._cancel_message is None:
                    exc = _asyncio.CancelledError()
                else:
                    exc = _asyncio.CancelledError(self._cancel_message)
                self._finish_exception(exc)
            else:
                results = self._results
                self._children = []
                self._results = []
                self.set_result(results)


def gather(*aws: Any, return_exceptions: bool = False) -> Future:
    if not aws:
        result = get_event_loop().create_future()
        result.set_result([])
        return result
    by_arg: dict[Any, Future] = {}
    positions: dict[Future, list[int]] = {}
    children: list[Future] = []
    loop = None
    for index, awaitable in enumerate(aws):
        if awaitable not in by_arg:
            child = ensure_future(awaitable, loop=loop)
            if loop is None:
                loop = _get_loop(child)
            by_arg[awaitable] = child
            positions[child] = []
        child = by_arg[awaitable]
        children.append(child)
        positions[child].append(index)
    outer = _GatheringFuture(children, len(positions), return_exceptions, loop)
    for child, indices in positions.items():
        if child.done():
            outer._child_done(indices, child)
        else:
            child.add_done_callback(functools.partial(outer._child_done, indices))
    return outer

async def _wait_one(queue: "Queue", timeout: float | None) -> Any:
    if timeout is None:
        task = await queue.get()
    else:
        task = await wait_for(queue.get(), timeout)
    return await task

class _AsCompletedIterator:
    def __init__(
        self,
        tasks: list[Future],
        queue: "Queue",
        timeout: float | None,
    ) -> None:
        self._tasks = tasks
        self._queue = queue
        self._timeout = timeout
        self._remaining = len(tasks)
        if timeout is None:
            self._deadline: float | None = None
        else:
            self._deadline = _time.monotonic() + max(0.0, float(timeout))

    def __iter__(self) -> "_AsCompletedIterator":
        return self

    def __next__(self) -> Any:
        if self._remaining <= 0:
            raise StopIteration
        self._remaining -= 1
        timeout: float | None
        if self._deadline is None:
            timeout = None
        else:
            timeout = self._deadline - _time.monotonic()
            if timeout < 0.0:
                timeout = 0.0
        return _wait_one(self._queue, timeout)

    # --- async iterator protocol (CPython 3.13+) ---
    if _VERSION_INFO >= (3, 13):

        def __aiter__(self) -> "_AsCompletedIterator":
            return self

        async def __anext__(self) -> Any:
            if self._remaining <= 0:
                raise StopAsyncIteration
            self._remaining -= 1
            timeout: float | None
            if self._deadline is None:
                timeout = None
            else:
                timeout = self._deadline - _time.monotonic()
                if timeout < 0.0:
                    timeout = 0.0
            return await _wait_one(self._queue, timeout)

def as_completed(aws: Iterable[Any], timeout: float | None = None) -> Iterator[Any]:
    tasks = [ensure_future(aw) for aw in aws]
    if timeout is None:
        normalized_timeout: float | None = None
    else:
        normalized_timeout = float(timeout)
    queue: Queue = _asyncio.Queue()

    def _enqueue(task: Future, _queue: "Queue" = queue) -> None:
        if not _queue.full():
            _queue.put_nowait(task)

    _asyncio_tasks_add_done_callback(tasks, _enqueue)

    return _AsCompletedIterator(tasks, queue, normalized_timeout)


__all__ = [
    "ALL_COMPLETED",
    "FIRST_COMPLETED",
    "FIRST_EXCEPTION",
    "GenericAlias",
    "Task",
    "all_tasks",
    "as_completed",
    "base_tasks",
    "concurrent",
    "contextvars",
    "coroutines",
    "create_eager_task_factory",
    "create_task",
    "current_task",
    "eager_task_factory",
    "ensure_future",
    "events",
    "exceptions",
    "functools",
    "futures",
    "gather",
    "inspect",
    "itertools",
    "run_coroutine_threadsafe",
    "shield",
    "sleep",
    "timeouts",
    "types",
    "wait",
    "wait_for",
    "warnings",
    "weakref",
]
if _EXPOSE_GRAPH:
    __all__.extend(["capture_call_graph", "format_call_graph", "print_call_graph"])

globals().pop("_require_intrinsic", None)
