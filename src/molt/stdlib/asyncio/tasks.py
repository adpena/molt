"""Task, runner, timeout, and wait/gather authority for ``asyncio.tasks``."""

from __future__ import annotations

import asyncio as _asyncio
import concurrent
import contextvars
from dataclasses import dataclass
import functools
import inspect
import itertools
import sys as _sys
import time as _time
import types as _types
import warnings
import weakref
from typing import TYPE_CHECKING, Any, Callable, Iterable, Iterator

from _intrinsics import require_intrinsic as _require_intrinsic
from ._debug import (
    _debug_exc_state,
    _debug_task_summary,
    _debug_tasks_enabled,
    _debug_write,
)

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from .futures import Future
from asyncio import (
    TimeoutError,
    _DEBUG_ASYNCIO_SHUTDOWN,
    _EXPOSE_GRAPH,
    _VERSION_INFO,
    _asyncio_cancel_pending_tasks,
    _asyncio_future_transfer,
    _asyncio_taskgroup_on_task_done,
    _asyncio_tasks_add_done_callback,
    _is_cancelled_exc,
    _require_asyncio_intrinsic,
    _task_registry_contains,
    _task_registry_current_for_loop,
    _task_registry_get,
    _task_registry_move,
    _task_registry_pop,
    _task_registry_set,
    _event_waiters_register,
    _event_waiters_unregister,
    _event_waiters_cleanup_token,
    iscoroutine,
    molt_async_sleep,
    molt_asyncio_future_cancelled,
    molt_asyncio_future_done,
    molt_asyncio_gather_new,
    molt_asyncio_taskgroup_request_cancel,
    molt_asyncio_task_cancel_apply,
    molt_asyncio_task_last_exception_clear,
    molt_asyncio_task_registry_live_set,
    molt_asyncio_task_uncancel_apply,
    molt_asyncio_to_thread,
    molt_asyncio_wait_for_new,
    molt_asyncio_wait_new,
    molt_cancel_token_cancel,
    molt_cancel_token_clone,
    molt_cancel_token_drop,
    molt_cancel_token_get_current,
    molt_cancel_token_is_cancelled,
    molt_cancel_token_new,
    molt_cancel_token_set_current,
    molt_spawn,
    molt_task_register_token_owned,
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
    molt_spawn(task)

class CancellationToken:
    def __init__(self) -> None:
        self._token = int(molt_cancel_token_new(None))
        self._owned = True

    @classmethod
    def detached(cls) -> "CancellationToken":
        token = cls()
        old_id = token._token
        token._token = int(molt_cancel_token_new(-1))
        molt_cancel_token_drop(old_id)
        return token

    def child(self) -> "CancellationToken":
        token = CancellationToken()
        old_id = token._token
        token._token = int(molt_cancel_token_new(self._token))
        molt_cancel_token_drop(old_id)
        return token

    def cancelled(self) -> bool:
        return bool(molt_cancel_token_is_cancelled(self._token))

    def cancel(self) -> None:
        molt_cancel_token_cancel(self._token)

    def set_current(self) -> "CancellationToken":
        prev_id = int(molt_cancel_token_set_current(self._token))
        return _wrap_existing_token(prev_id, False)

    def token_id(self) -> int:
        return int(self._token)

    def __del__(self) -> None:
        if getattr(self, "_owned", False):
            molt_cancel_token_drop(int(self._token))

def _wrap_existing_token(token_id: int, owned: bool) -> CancellationToken:
    token = CancellationToken()
    old_id = token._token
    token._token = int(token_id)
    token._owned = bool(owned)
    if owned:
        molt_cancel_token_clone(int(token_id))
    if old_id != token_id:
        molt_cancel_token_drop(int(old_id))
    return token

def _swap_current_token(token: CancellationToken) -> int:
    if molt_cancel_token_set_current is not None:  # type: ignore[name-defined]
        return molt_cancel_token_set_current(token.token_id())  # type: ignore[name-defined]
    return 0

def _restore_token_id(token_id: int) -> None:
    if molt_cancel_token_set_current is not None:  # type: ignore[name-defined]
        molt_cancel_token_set_current(token_id)  # type: ignore[name-defined]
    return None

def _current_token_id() -> int:
    if molt_cancel_token_get_current is not None:  # type: ignore[name-defined]
        return molt_cancel_token_get_current()  # type: ignore[name-defined]
    return 0

def _future_done(task: Any) -> bool:
    if isinstance(task, Future):
        return bool(molt_asyncio_future_done(task._fut_handle))
    done_fn = getattr(task, "done", None)
    if callable(done_fn):
        return done_fn()
    return False

def _future_cancelled(task: Any) -> bool:
    if isinstance(task, Future):
        return bool(molt_asyncio_future_cancelled(task._fut_handle))
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
    _runner_spawned: bool

    def __init__(
        self,
        coro: Any,
        *,
        loop: "EventLoop | None" = None,
        name: str | None = None,
        context: Any | None = None,
        _spawn_runner: bool = True,
    ) -> None:
        super().__init__()
        self._coro = coro
        task_dict = getattr(self, "__dict__", None)
        if isinstance(task_dict, dict):
            task_dict["_coro"] = coro
        self._runner_task: Any | None = None
        self._token = CancellationToken()
        if loop is not None:
            self._loop = loop
        self._name = name or _next_task_name()
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
        self._runner_spawned = _spawn_runner
        token_id = self._token.token_id()
        if molt_task_register_token_owned is not None:  # type: ignore[name-defined]
            molt_task_register_token_owned(self._coro, token_id)  # type: ignore[name-defined]
        if _spawn_runner:
            prev_id = _swap_current_token(self._token)
            try:
                runner = self._runner(self._coro)
                self._runner_task = runner
                if molt_task_register_token_owned is not None:  # type: ignore[name-defined]
                    molt_task_register_token_owned(  # type: ignore[name-defined]
                        runner, token_id
                    )
                spawn(runner)
            finally:
                _restore_token_id(prev_id)

    def _rebind_token(self, token: CancellationToken) -> None:
        old_token = self._token
        old_id = old_token.token_id()
        new_id = token.token_id()
        if new_id == old_id:
            return
        if _task_registry_get(old_id) is self:
            _task_registry_move(old_id, new_id)
        else:
            _task_registry_set(new_id, self)
        self._token = token
        _set_ctx = getattr(_contextvars, "_set_context_for_token", None)
        if callable(_set_ctx):
            _set_ctx(new_id, self._context)
        _clear_ctx = getattr(_contextvars, "_clear_context_for_token", None)
        if callable(_clear_ctx):
            _clear_ctx(old_id)

    def cancel(self, msg: Any | None = None) -> bool:
        if molt_asyncio_future_done(self._fut_handle):
            return False
        self._cancel_requested += 1
        if msg is None:
            self._cancel_message = None
        else:
            self._cancel_message = msg
        if _debug_tasks_enabled():
            token_id = self._token.token_id()
            _debug_write(
                "asyncio_task_cancel token={token} msg={msg!r}".format(
                    token=token_id, msg=msg
                )
            )
        self._token.cancel()
        _require_asyncio_intrinsic(
            molt_asyncio_task_cancel_apply, "asyncio_task_cancel_apply"
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
        self._name = value

    def get_context(self) -> Any:
        return self._context

    def cancelling(self) -> int:
        return self._cancel_requested

    def uncancel(self) -> int:
        if self._cancel_requested <= 0:
            return 0
        self._cancel_requested -= 1
        if self._cancel_requested == 0:
            self._cancel_message = None
            _require_asyncio_intrinsic(
                molt_asyncio_task_uncancel_apply, "asyncio_task_uncancel_apply"
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
        if _debug_tasks_enabled():
            token_id = self._token.token_id()
            coro_name = getattr(coro, "__qualname__", None) or getattr(
                coro, "__name__", None
            )
            if coro_name is None:
                coro_name = type(coro).__name__
            _debug_write(f"asyncio_task_start token={token_id} coro={coro_name}")
        try:
            result = await coro
        except BaseException as err:
            exc = err
            if _debug_tasks_enabled():
                token_id = self._token.token_id()
                _debug_write(
                    "asyncio_task_exc token={token_id} type={exc_type}".format(
                        token_id=token_id,
                        exc_type=type(err).__name__,
                    )
                )
        if exc is None:
            if not molt_asyncio_future_done(self._fut_handle):
                self.set_result(result)
                if _debug_tasks_enabled():
                    token_id = self._token.token_id()
                    _debug_write(f"asyncio_task_done token={token_id}")
            molt_asyncio_task_last_exception_clear(coro)
        else:
            if not molt_asyncio_future_done(self._fut_handle):
                self.set_exception(exc)
        _cleanup_event_waiters_for_token(self._token.token_id())
        _debug_exc_state("task_runner_after_cleanup_event_waiters")
        _task_registry_pop(self._token.token_id())
        _debug_exc_state("task_runner_after_task_registry_pop")
        if extra_token_id is not None:
            _task_registry_pop(extra_token_id)
            _debug_exc_state("task_runner_after_extra_task_registry_pop")
        _contextvars._clear_context_for_token(  # type: ignore[unresolved-attribute]
            self._token.token_id()
        )
        _debug_exc_state("task_runner_after_clear_context")

    def __repr__(self) -> str:
        if molt_asyncio_future_cancelled(self._fut_handle):
            state = "cancelled"
        elif molt_asyncio_future_done(self._fut_handle):
            state = "finished"
        else:
            state = "pending"
        return f"<Task {self._name} {state}>"

    def __await__(self) -> Any:
        if molt_asyncio_future_done(self._fut_handle):
            return self._wait().__await__()
        waiter = Future()

        def _transfer(done: Future) -> None:
            if waiter.done():
                return
            try:
                if _asyncio_future_transfer(done, waiter):
                    return
                if hasattr(done, "cancelled") and done.cancelled():
                    cancel_msg = getattr(done, "_cancel_message", None)
                    waiter.cancel(cancel_msg)
                    return
                exc = done.exception()
                if exc is not None:
                    waiter.set_exception(exc)
                    return
                waiter.set_result(done.result())
            except BaseException as exc:
                if not waiter.done():
                    waiter.set_exception(exc)

        self.add_done_callback(lambda _fut: _transfer(_fut))
        return waiter.__await__()

class TaskGroup:
    def __init__(self) -> None:
        self._tasks: set[Task] = set()
        self._errors: list[BaseException] = []
        self._loop: EventLoop | None = None
        self._entered = False
        self._exiting = False
        self._cancel_handle: Handle | None = None

    async def __aenter__(self) -> "TaskGroup":
        self._loop = get_running_loop()
        self._entered = True
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        self._exiting = True
        if exc is not None:
            self._cancel_all()
        await self._wait_tasks()
        if self._errors:
            if any(not isinstance(err, Exception) for err in self._errors):
                raise BaseExceptionGroup("unhandled errors in TaskGroup", self._errors)
            exceptions = [err for err in self._errors if isinstance(err, Exception)]
            raise ExceptionGroup("unhandled errors in TaskGroup", exceptions)
        return False

    def create_task(
        self, coro: Any, *, name: str | None = None, context: Any | None = None
    ) -> Task:
        if not self._entered:
            raise RuntimeError("TaskGroup has not been entered")
        loop = self._loop or get_running_loop()
        task = loop.create_task(coro, name=name, context=context)
        self._tasks.add(task)
        task.add_done_callback(self._on_task_done)
        return task

    def _on_task_done(self, task: Future) -> None:
        if _asyncio_taskgroup_on_task_done(self._tasks, self._errors, task):
            self._request_cancel()

    def _request_cancel(self) -> None:
        self._cancel_handle = _require_asyncio_intrinsic(
            molt_asyncio_taskgroup_request_cancel, "asyncio_taskgroup_request_cancel"
        )(self._loop, self._cancel_all, self._cancel_handle)

    async def _wait_tasks(self) -> None:
        if not self._tasks:
            return
        waiter = _require_asyncio_intrinsic(
            molt_asyncio_gather_new, "asyncio_gather_new"
        )(list(self._tasks), True)
        try:
            await waiter
        except BaseException:
            pass

    def _cancel_all(self) -> None:
        self._cancel_handle = None
        if not self._tasks:
            return
        _asyncio_cancel_pending_tasks(self._tasks)

class _Timeout:
    def __init__(self, when: float | None) -> None:
        self._when = when
        self._loop: EventLoop | None = None
        self._task: Task | None = None
        self._handle: TimerHandle | None = None
        self._timed_out = False

    def when(self) -> float | None:
        """Return the current deadline, or ``None`` if not set."""
        return self._when

    def reschedule(self, when: float | None) -> None:
        """Reschedule the timeout to *when* (absolute loop time), or disable if ``None``."""
        if self._task is None:
            raise RuntimeError("Timeout has not been entered")
        self._when = when
        # Cancel the old timer if one is pending.
        if self._handle is not None:
            cancel = getattr(self._handle, "cancel", None)
            if callable(cancel):
                cancel()
            self._handle = None
        # If no deadline, nothing more to do.
        if when is None:
            return
        loop = self._loop
        if loop is None:
            return
        delay = when - loop.time()
        if delay <= 0:
            self._timed_out = True
            self._task.cancel()
        else:
            self._handle = loop.call_later(delay, self._on_timeout)

    def expired(self) -> bool:
        """Return ``True`` if the timeout has expired (the inner body was cancelled)."""
        return self._timed_out

    def _on_timeout(self) -> None:
        if self._task is None or self._timed_out:
            return
        self._timed_out = True
        self._task.cancel()

    async def __aenter__(self) -> "_Timeout":
        self._loop = get_running_loop()
        self._task = current_task(self._loop)
        if self._when is None or self._task is None:
            return self
        delay = self._when - self._loop.time()
        if delay <= 0:
            self._timed_out = True
            self._task.cancel()
            return self
        self._handle = self._loop.call_later(delay, self._on_timeout)
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        if self._handle is not None:
            self._handle.cancel()
        if exc is None:
            return False
        if self._timed_out and _is_cancelled_exc(exc):
            if self._task is not None:
                self._task.uncancel()
            raise TimeoutError
        return False

class Runner:
    def __init__(
        self,
        *,
        debug: bool | None = None,
        loop_factory: Callable[[], "EventLoop"] | None = None,
    ) -> None:
        self._loop: EventLoop | None = None
        self._debug = debug
        self._loop_factory = loop_factory
        self._context: Any | None = None

    def __enter__(self) -> "Runner":
        if self._loop is None:
            if self._loop_factory is not None:
                self._loop = self._loop_factory()
            else:
                self._loop = new_event_loop()
            if self._debug is not None:
                self._loop.set_debug(self._debug)
            self._context = _contextvars.copy_context()
            set_event_loop(self._loop)
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    def get_loop(self) -> EventLoop:
        if self._loop is None:
            raise RuntimeError("Runner is not initialized")
        return self._loop

    def run(self, coro: Any, *, context: Any | None = None) -> Any:
        if self._loop is None:
            self.__enter__()
        loop = self.get_loop()
        if loop.is_running():
            raise RuntimeError("Runner loop is already running")
        if context is None:
            context = self._context
        task = Task(coro, loop=loop, context=context, _spawn_runner=False)
        try:
            loop.run_until_complete(task)
            result = task.result()
            if _DEBUG_ASYNCIO_SHUTDOWN:
                _debug_write(
                    "asyncio_runner_run_after_complete {summary}".format(
                        summary=_debug_task_summary(task)
                    )
                )
        except BaseException:
            _cancel_all_tasks(loop)
            shutdown = globals().get("molt_asyncgen_shutdown")
            if shutdown is not None:
                shutdown()
            raise
        _cancel_all_tasks(loop)
        shutdown = globals().get("molt_asyncgen_shutdown")
        if shutdown is not None:
            shutdown()
        return result

    def close(self) -> None:
        if self._loop is None:
            return
        if not self._loop.is_closed():
            if _DEBUG_ASYNCIO_SHUTDOWN:
                _debug_write("asyncio_runner_close_begin")
            _cancel_all_tasks(self._loop)
            shutdown = globals().get("molt_asyncgen_shutdown")
            if shutdown is not None:
                shutdown()
            self._loop.close()
            if _DEBUG_ASYNCIO_SHUTDOWN:
                _debug_write("asyncio_runner_close_end")
        set_event_loop(None)
        self._context = None

def run(
    awaitable: Any,
    *,
    debug: bool | None = None,
    loop_factory: Callable[[], "EventLoop"] | None = None,
) -> Any:
    if _get_running_loop() is not None:
        raise RuntimeError("asyncio.run() cannot be called from a running event loop")
    runner = Runner(debug=debug, loop_factory=loop_factory)
    exc: BaseException | None = None
    result: Any = None
    runner.__enter__()
    try:
        try:
            result = runner.run(awaitable)
        except BaseException as err:
            exc = err
    finally:
        try:
            runner.close()
        except BaseException as close_exc:
            if exc is None:
                exc = close_exc
    if exc is not None:
        raise exc
    return result

async def sleep(delay: float = 0.0, result: Any | None = None) -> Any:
    if delay <= 0:
        delay = 0.0
    else:
        delay = float(delay)
    fut = _require_asyncio_intrinsic(molt_async_sleep, "async_sleep")(delay, result)
    return await fut

async def to_thread(func: Any, /, *args: Any, **kwargs: Any) -> Any:
    args_tuple = args if args else None
    kwargs_dict = kwargs if kwargs else None
    return await molt_asyncio_to_thread(func, args_tuple, kwargs_dict)

async def shield(awaitable: Any) -> Any:
    fut: Future
    if isinstance(awaitable, Future):
        fut = awaitable
    else:
        root = CancellationToken()
        prev_id = _swap_current_token(root)
        try:
            fut = ensure_future(awaitable)
        finally:
            _restore_token_id(prev_id)
    current_id = _current_token_id()
    if isinstance(fut, Task):
        token = getattr(fut, "_token", None)
        token_id = token.token_id() if token is not None else None
        if token_id == current_id:
            shield_token = CancellationToken.detached()
            fut._rebind_token(shield_token)
            if molt_task_register_token_owned is not None:  # type: ignore[name-defined]
                molt_task_register_token_owned(  # type: ignore[name-defined]
                    fut._coro, shield_token.token_id()
                )
            setattr(fut, "__molt_shield_token__", shield_token)

            def _clear_shield_token(done: Future) -> None:
                if hasattr(done, "__molt_shield_token__"):
                    delattr(done, "__molt_shield_token__")

            fut.add_done_callback(_clear_shield_token)
    try:
        return await fut
    except BaseException as exc:
        if _is_cancelled_exc(exc):
            raise
        raise

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
    if isinstance(awaitable, Future):
        return awaitable
    if loop is None:
        try:
            loop = get_running_loop()
        except RuntimeError:
            loop = get_event_loop()
    return Task(awaitable, loop=loop)

def run_coroutine_threadsafe(coro: Any, loop: EventLoop) -> Future:
    fut = Future()

    def _schedule() -> None:
        try:
            task = loop.create_task(coro)
        except BaseException as exc:
            fut.set_exception(exc)
            return

        def _transfer(done: Future) -> None:
            try:
                fut.set_result(done.result())
            except BaseException as exc:
                fut.set_exception(exc)

        task.add_done_callback(_transfer)

    try:
        loop.call_soon_threadsafe(_schedule)
    except BaseException as exc:
        fut.set_exception(exc)
    return fut

def wrap_future(fut: Any, *, loop: EventLoop | None = None) -> Future:
    if isinstance(fut, Future):
        return fut
    if isinstance(fut, Task):
        return fut
    if loop is None:
        try:
            loop = get_running_loop()
        except RuntimeError:
            loop = get_event_loop()
    proxy = Future()

    def _transfer(done_obj: Any) -> None:
        try:
            if _asyncio_future_transfer(done_obj, proxy):
                return
            if hasattr(done_obj, "cancelled") and done_obj.cancelled():
                proxy.cancel()
                return
            if hasattr(done_obj, "exception"):
                exc = done_obj.exception()
                if exc is not None:
                    proxy.set_exception(exc)
                    return
            if hasattr(done_obj, "result"):
                proxy.set_result(done_obj.result())
                return
        except BaseException as exc:
            if not proxy.done():
                proxy.set_exception(exc)
            return
        if not proxy.done():
            proxy.set_result(None)

    def _schedule_transfer(done_obj: Any) -> None:
        try:
            loop.call_soon_threadsafe(_transfer, done_obj)
        except BaseException:
            _transfer(done_obj)

    try:
        if hasattr(fut, "add_done_callback"):
            fut.add_done_callback(_schedule_transfer)
        else:
            _schedule_transfer(fut)
    except BaseException as exc:
        proxy.set_exception(exc)
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
        molt_asyncio_task_registry_live_set, "asyncio_task_registry_live_set"
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

async def wait(
    aws: Any,
    timeout: float | None = None,
    return_when: object = ALL_COMPLETED,
) -> tuple[set[Future], set[Future]]:
    get_running_loop()
    aws_list = list(aws)
    tasks: list[Future] = []
    for aw in aws_list:
        if iscoroutine(aw):
            raise TypeError("Passing coroutines is forbidden, use tasks explicitly.")
        tasks.append(ensure_future(aw))
    if not tasks:
        raise ValueError("asyncio.wait() requires at least one awaitable")
    if return_when not in (ALL_COMPLETED, FIRST_COMPLETED, FIRST_EXCEPTION):
        raise ValueError("Invalid return_when value")
    if return_when is ALL_COMPLETED:
        return_code = 0
    elif return_when is FIRST_COMPLETED:
        return_code = 1
    else:
        return_code = 2
    waiter = _require_asyncio_intrinsic(molt_asyncio_wait_new, "asyncio_wait_new")(
        tasks, timeout, return_code
    )
    return await waiter

async def wait_for(awaitable: Any, timeout: float | None) -> Any:
    fut = ensure_future(awaitable)
    waiter = _require_asyncio_intrinsic(
        molt_asyncio_wait_for_new, "asyncio_wait_for_new"
    )(fut, timeout)
    return await waiter

def timeout(delay: float | None) -> _Timeout:
    if delay is None:
        return _Timeout(None)
    loop = get_running_loop()
    return _Timeout(loop.time() + float(delay))

def timeout_at(when: float) -> _Timeout:
    return _Timeout(float(when))

async def gather(*aws: Any, return_exceptions: bool = False) -> list[Any]:
    if not aws:
        return []
    tasks = [ensure_future(aw) for aw in aws]
    waiter = _require_asyncio_intrinsic(molt_asyncio_gather_new, "asyncio_gather_new")(
        tasks, return_exceptions
    )
    return await waiter

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
    queue: Queue = Queue()

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
