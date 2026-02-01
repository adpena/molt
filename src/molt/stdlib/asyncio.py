"""Capability-gated asyncio shim for Molt."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Callable, Iterable, Iterator
from dataclasses import dataclass
import builtins as _builtins
from collections import deque as _deque
import heapq as _heapq
import inspect as _inspect
import logging as _logging
import os as _os
import sys as _sys
import time as _time
import traceback as _traceback
import errno as _errno
import socket as _socket
import types as _types

import contextvars as _contextvars

from molt.concurrency import CancellationToken, spawn

_VERSION_INFO = getattr(_sys, "version_info", (3, 12, 0, "final", 0))
_IS_WINDOWS = _os.name == "nt"
_EXPOSE_EVENT_LOOP = _VERSION_INFO >= (3, 13)
_EXPOSE_WINDOWS_POLICIES = _IS_WINDOWS
_EXPOSE_QUEUE_SHUTDOWN = _VERSION_INFO >= (3, 13)
_EXPOSE_GRAPH = _VERSION_INFO >= (3, 14)

_BASE_ALL = [
    "AbstractEventLoop",
    "AbstractEventLoopPolicy",
    "AbstractServer",
    "BaseEventLoop",
    "CancelledError",
    "Condition",
    "DefaultEventLoopPolicy",
    "Event",
    "Future",
    "Handle",
    "InvalidStateError",
    "IncompleteReadError",
    "LifoQueue",
    "LimitOverrunError",
    "Lock",
    "PriorityQueue",
    "FIRST_COMPLETED",
    "FIRST_EXCEPTION",
    "ALL_COMPLETED",
    "Queue",
    "QueueEmpty",
    "QueueFull",
    "BrokenBarrierError",
    "Barrier",
    "Runner",
    "SelectorEventLoop",
    "Semaphore",
    "BoundedSemaphore",
    "SendfileNotAvailableError",
    "Server",
    "StreamReader",
    "StreamWriter",
    "Task",
    "TaskGroup",
    "TimerHandle",
    "TimeoutError",
    "create_eager_task_factory",
    "eager_task_factory",
    "all_tasks",
    "as_completed",
    "create_task",
    "create_subprocess_exec",
    "create_subprocess_shell",
    "current_task",
    "ensure_future",
    "gather",
    "get_event_loop",
    "get_event_loop_policy",
    "get_child_watcher",
    "get_running_loop",
    "isfuture",
    "iscoroutine",
    "iscoroutinefunction",
    "new_event_loop",
    "open_unix_connection",
    "open_connection",
    "run",
    "run_coroutine_threadsafe",
    "start_server",
    "start_unix_server",
    "set_event_loop_policy",
    "set_event_loop",
    "shield",
    "sleep",
    "subprocess",
    "timeout",
    "timeout_at",
    "to_thread",
    "wrap_future",
    "wait",
    "wait_for",
    "set_child_watcher",
    "AbstractChildWatcher",
    "FastChildWatcher",
    "PidfdChildWatcher",
    "SafeChildWatcher",
    "ThreadedChildWatcher",
]

__all__ = list(_BASE_ALL)
if _EXPOSE_EVENT_LOOP:
    __all__.append("EventLoop")
if _EXPOSE_QUEUE_SHUTDOWN:
    __all__.append("QueueShutDown")
if _EXPOSE_GRAPH:
    __all__.extend(
        [
            "capture_call_graph",
            "format_call_graph",
            "print_call_graph",
            "future_add_to_awaited_by",
            "future_discard_from_awaited_by",
        ]
    )
if _EXPOSE_WINDOWS_POLICIES:
    __all__.extend(
        [
            "ProactorEventLoop",
            "WindowsProactorEventLoopPolicy",
            "WindowsSelectorEventLoopPolicy",
        ]
    )

if TYPE_CHECKING:

    def molt_async_sleep(_delay: float = 0.0, _result: Any | None = None) -> Any:
        raise NotImplementedError

    def molt_block_on(awaitable: Any) -> Any:
        raise NotImplementedError

    def molt_asyncgen_shutdown() -> None:
        raise NotImplementedError

    def molt_cancel_token_set_current(_token_id: int) -> int:
        raise NotImplementedError

    def molt_promise_new() -> Any:
        raise NotImplementedError

    def molt_promise_set_exception(_promise: Any, _exc: Any) -> None:
        raise NotImplementedError

    def molt_promise_set_result(_promise: Any, _value: Any) -> None:
        raise NotImplementedError

    def molt_cancel_token_get_current() -> int:
        raise NotImplementedError

    def molt_task_register_token_owned(_task: Any, _token_id: int) -> None:
        raise NotImplementedError

    def molt_future_cancel(_future: Any) -> None:
        raise NotImplementedError

    def molt_thread_submit(_func: Any, _args: Any, _kwargs: Any) -> Any:
        raise NotImplementedError


def _mark_builtin(fn: Any) -> None:
    func = _molt_function_set_builtin
    if func is None:
        return None
    try:
        func(fn)
    except Exception:
        return None


_builtin_cancelled = getattr(_builtins, "CancelledError", None)
if _builtin_cancelled is None:

    class CancelledError(BaseException):
        pass

    _builtins.CancelledError = CancelledError  # type: ignore[attr-defined]
else:
    CancelledError = _builtin_cancelled


def _is_cancelled_exc(exc: BaseException) -> bool:
    if isinstance(exc, CancelledError):
        return True
    return type(exc).__name__ == "CancelledError"


class InvalidStateError(Exception):
    pass


TimeoutError = _builtins.TimeoutError


class QueueEmpty(Exception):
    pass


class QueueFull(Exception):
    pass


class _QueueShutDown(Exception):
    pass


class BrokenBarrierError(RuntimeError):
    pass


class LimitOverrunError(Exception):
    def __init__(self, message: str, consumed: int) -> None:
        super().__init__(message)
        self.consumed = consumed


class SendfileNotAvailableError(RuntimeError):
    pass


FIRST_COMPLETED = object()
FIRST_EXCEPTION = object()
ALL_COMPLETED = object()


def isfuture(obj: Any) -> bool:
    return isinstance(obj, Future)


def iscoroutine(obj: Any) -> bool:
    return _inspect.iscoroutine(obj)


def iscoroutinefunction(func: Any) -> bool:
    return _inspect.iscoroutinefunction(func)


class Future:
    def __init__(self) -> None:
        self._done = False
        self._cancelled = False
        self._result: Any = None
        self._exception: BaseException | None = None
        self._cancel_message: Any | None = None
        self._molt_event_owner: Event | None = None
        self._molt_event_token_id: int | None = None
        if _EXPOSE_GRAPH:
            self._asyncio_awaited_by: set["Future"] | None = None
        self._callbacks: list[tuple[Callable[["Future"], Any], Any | None]] = []
        self._molt_promise: Any | None = None
        try:
            self._molt_promise = molt_promise_new()
        except Exception:
            self._molt_promise = None
        if _DEBUG_ASYNCIO_PROMISE:
            _debug_write(
                "asyncio_promise_new ok={ok} promise={promise}".format(
                    ok=self._molt_promise is not None,
                    promise=self._molt_promise,
                )
            )

    def cancel(self, msg: Any | None = None) -> bool:
        if self._done:
            return False
        if _DEBUG_TASKS:
            _debug_write(
                "asyncio_future_cancel type={typ} msg={msg!r}".format(
                    typ=type(self).__name__, msg=msg
                )
            )
        self._cancelled = True
        self._exception = None
        self._cancel_message = None
        if msg is not None:
            try:
                self._exception = CancelledError(msg)
            except Exception:
                self._exception = CancelledError()
            self._cancel_message = msg
        self._done = True
        if self._molt_promise is not None:
            exc_obj: Any = self._exception
            if exc_obj is None:
                exc_obj = CancelledError
            try:
                molt_promise_set_exception(self._molt_promise, exc_obj)
            except Exception:
                pass
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
            if _DEBUG_ASYNCIO_EXC:
                try:
                    _debug_write(
                        "future_exception_type={name}".format(
                            name=type(self._exception).__name__
                        )
                    )
                except Exception:
                    pass
            _debug_exc_state("future_result_before_raise")
            raise self._exception
            _debug_exc_state("future_result_after_raise")
        return self._result

    def exception(self) -> BaseException | None:
        if not self._done:
            raise InvalidStateError("Result is not ready")
        if self._cancelled:
            if self._exception is not None:
                raise self._exception
            raise CancelledError
        return self._exception

    def add_done_callback(
        self, fn: Callable[["Future"], Any], *, context: Any | None = None
    ) -> None:
        if context is None:
            try:
                context = _contextvars.copy_context()
            except Exception:
                context = None
        if self._done:
            self._run_callback(fn, context)
            return None
        self._callbacks.append((fn, context))
        return None

    def set_result(self, result: Any) -> None:
        if self._done:
            raise InvalidStateError("Result is already set")
        self._result = result
        self._done = True
        if self._molt_promise is not None:
            try:
                molt_promise_set_result(self._molt_promise, result)
            except Exception:
                if _DEBUG_ASYNCIO_PROMISE:
                    _debug_write("asyncio_promise_set_result_error")
        self._invoke_callbacks()

    def set_exception(self, exception: BaseException) -> None:
        if self._done:
            raise InvalidStateError("Result is already set")
        self._exception = exception
        if _is_cancelled_exc(exception):
            self._cancelled = True
        self._done = True
        if self._molt_promise is not None:
            try:
                molt_promise_set_exception(self._molt_promise, exception)
            except Exception:
                if _DEBUG_ASYNCIO_PROMISE:
                    _debug_write("asyncio_promise_set_exception_error")
        self._invoke_callbacks()

    def _invoke_callbacks(self) -> None:
        callbacks = self._callbacks
        self._callbacks = []
        idx = 0
        while idx < len(callbacks):
            fn, ctx = callbacks[idx]
            self._run_callback(fn, ctx)
            idx += 1

    def _run_callback(self, fn: Callable[["Future"], Any], context: Any | None) -> None:
        try:
            if context is not None:
                context.run(fn, self)
            else:
                fn(self)
        except Exception:
            pass

    async def _wait(self) -> Any:
        while not self._done:
            await molt_async_sleep(0.0)
        return self.result()

    def __await__(self) -> Any:
        async def _wrapped() -> Any:
            waiter = None
            if _EXPOSE_GRAPH:
                waiter = _TASKS.get(_current_token_id())
                if isinstance(waiter, Future):
                    future_add_to_awaited_by(self, waiter)
            try:
                if self._molt_promise is not None:
                    if _DEBUG_ASYNCIO_PROMISE:
                        _debug_write("asyncio_promise_await")
                    return await self._molt_promise
                if _DEBUG_ASYNCIO_PROMISE:
                    _debug_write("asyncio_promise_fallback_wait")
                return await self._wait()
            finally:
                if _EXPOSE_GRAPH and isinstance(waiter, Future):
                    future_discard_from_awaited_by(self, waiter)

        return _wrapped().__await__()

    def __repr__(self) -> str:
        if self._cancelled:
            state = "cancelled"
        elif self._done:
            state = "finished"
        else:
            state = "pending"
        return f"<Future {state}>"


def future_add_to_awaited_by(fut: Any, waiter: Any) -> None:
    if isinstance(fut, Future) and isinstance(waiter, Future):
        if fut._asyncio_awaited_by is None:
            fut._asyncio_awaited_by = set()
        fut._asyncio_awaited_by.add(waiter)


def future_discard_from_awaited_by(fut: Any, waiter: Any) -> None:
    if isinstance(fut, Future) and isinstance(waiter, Future):
        if fut._asyncio_awaited_by is not None:
            fut._asyncio_awaited_by.discard(waiter)


_TASKS: dict[int, "Task"] = {}
_EVENT_WAITERS: dict[int, list[Future]] = {}


def _debug_gather_enabled() -> bool:
    try:
        return _os.getenv("MOLT_DEBUG_GATHER") == "1"
    except Exception:
        return False


_DEBUG_GATHER = _debug_gather_enabled()


def _debug_wait_for_enabled() -> bool:
    try:
        return _os.getenv("MOLT_DEBUG_WAIT_FOR") == "1"
    except Exception:
        return False


_DEBUG_WAIT_FOR = _debug_wait_for_enabled()


def _debug_tasks_enabled() -> bool:
    try:
        return _os.getenv("MOLT_DEBUG_TASKS") == "1"
    except Exception:
        return False


_DEBUG_TASKS = _debug_tasks_enabled()


def _debug_asyncio_promise_enabled() -> bool:
    try:
        return _os.getenv("MOLT_DEBUG_ASYNCIO_PROMISE") == "1"
    except Exception:
        return False


_DEBUG_ASYNCIO_PROMISE = _debug_asyncio_promise_enabled()


def _debug_asyncio_exc_enabled() -> bool:
    try:
        return _os.getenv("MOLT_DEBUG_ASYNCIO_EXC") == "1"
    except Exception:
        return False


_DEBUG_ASYNCIO_EXC = _debug_asyncio_exc_enabled()


def _debug_asyncio_condition_enabled() -> bool:
    try:
        return _os.getenv("MOLT_DEBUG_ASYNCIO_CONDITION") == "1"
    except Exception:
        return False


_DEBUG_ASYNCIO_CONDITION = _debug_asyncio_condition_enabled()

_UNSET = object()
_PENDING = 0x7FFD_0000_0000_0000
_PROC_STDIO_INHERIT = 0
_PROC_STDIO_PIPE = 1
_PROC_STDIO_DEVNULL = 2
_WAIT_TIMEOUT_EPS = 1e-6


def _require_asyncio_intrinsic(
    fn: Callable[..., Any] | None, name: str
) -> Callable[..., Any]:
    if fn is None:
        raise RuntimeError(f"asyncio intrinsic not available: {name}")
    return fn


async def _io_wait(fd: int, events: int, timeout: float | None = None) -> Any:
    if _molt_io_wait_new is None:
        raise NotImplementedError("I/O polling unavailable")
    return await _require_asyncio_intrinsic(_molt_io_wait_new, "io_wait_new")(
        fd, events, timeout
    )


def _load_intrinsic(name: str) -> Any | None:
    direct = globals().get(name)
    if direct is not None:
        return direct
    return getattr(_builtins, name, None)


_molt_io_wait_new = _load_intrinsic("_molt_io_wait_new")
molt_asyncgen_shutdown: Callable[[], Any] | None = _load_intrinsic(
    "molt_asyncgen_shutdown"
)
_molt_module_new = _load_intrinsic("_molt_module_new")
_molt_function_set_builtin: Callable[[Any], Any] | None = _load_intrinsic(
    "_molt_function_set_builtin"
)
_molt_future_cancel_msg: Callable[[Any, Any], None] | None = _load_intrinsic(
    "molt_future_cancel_msg"
)
if _molt_future_cancel_msg is None:
    try:
        _molt_future_cancel_msg = molt_future_cancel_msg  # type: ignore[name-defined]  # noqa: F821
    except Exception:
        _molt_future_cancel_msg = None
_molt_future_cancel_clear: Callable[[Any], None] | None = _load_intrinsic(
    "molt_future_cancel_clear"
)
if _molt_future_cancel_clear is None:
    try:
        _molt_future_cancel_clear = molt_future_cancel_clear  # type: ignore[name-defined]  # noqa: F821
    except Exception:
        _molt_future_cancel_clear = None
_molt_thread_submit: Callable[[Any, Any, Any], Any] | None = _load_intrinsic(
    "molt_thread_submit"
)
_molt_exception_pending: Callable[[], int] | None = _load_intrinsic(
    "molt_exception_pending"
)
_molt_exception_last: Callable[[], Any] | None = _load_intrinsic("molt_exception_last")
_molt_process_spawn: Callable[[Any, Any, Any, Any, Any, Any], Any] | None = (
    _load_intrinsic("molt_process_spawn")
)
_molt_process_wait_future: Callable[[Any], Any] | None = _load_intrinsic(
    "molt_process_wait_future"
)
_molt_process_pid: Callable[[Any], int] | None = _load_intrinsic("molt_process_pid")
_molt_process_returncode: Callable[[Any], int | None] | None = _load_intrinsic(
    "molt_process_returncode"
)
_molt_process_kill: Callable[[Any], None] | None = _load_intrinsic("molt_process_kill")
_molt_process_terminate: Callable[[Any], None] | None = _load_intrinsic(
    "molt_process_terminate"
)
_molt_process_stdin: Callable[[Any], Any] | None = _load_intrinsic("molt_process_stdin")
_molt_process_stdout: Callable[[Any], Any] | None = _load_intrinsic(
    "molt_process_stdout"
)
_molt_process_stderr: Callable[[Any], Any] | None = _load_intrinsic(
    "molt_process_stderr"
)
_molt_process_drop: Callable[[Any], None] | None = _load_intrinsic("molt_process_drop")
if _molt_process_spawn is None:
    try:
        _molt_process_spawn = molt_process_spawn  # type: ignore[name-defined]  # noqa: F821
    except Exception:
        _molt_process_spawn = None
if _molt_process_wait_future is None:
    try:
        _molt_process_wait_future = molt_process_wait_future  # type: ignore[name-defined]  # noqa: F821
    except Exception:
        _molt_process_wait_future = None
if _molt_process_pid is None:
    try:
        _molt_process_pid = molt_process_pid  # type: ignore[name-defined]  # noqa: F821
    except Exception:
        _molt_process_pid = None
if _molt_process_returncode is None:
    try:
        _molt_process_returncode = molt_process_returncode  # type: ignore[name-defined]  # noqa: F821
    except Exception:
        _molt_process_returncode = None
if _molt_process_kill is None:
    try:
        _molt_process_kill = molt_process_kill  # type: ignore[name-defined]  # noqa: F821
    except Exception:
        _molt_process_kill = None
if _molt_process_terminate is None:
    try:
        _molt_process_terminate = molt_process_terminate  # type: ignore[name-defined]  # noqa: F821
    except Exception:
        _molt_process_terminate = None
if _molt_process_stdin is None:
    try:
        _molt_process_stdin = molt_process_stdin  # type: ignore[name-defined]  # noqa: F821
    except Exception:
        _molt_process_stdin = None
if _molt_process_stdout is None:
    try:
        _molt_process_stdout = molt_process_stdout  # type: ignore[name-defined]  # noqa: F821
    except Exception:
        _molt_process_stdout = None
if _molt_process_stderr is None:
    try:
        _molt_process_stderr = molt_process_stderr  # type: ignore[name-defined]  # noqa: F821
    except Exception:
        _molt_process_stderr = None
if _molt_process_drop is None:
    try:
        _molt_process_drop = molt_process_drop  # type: ignore[name-defined]  # noqa: F821
    except Exception:
        _molt_process_drop = None
_molt_stream_new: Callable[[int], Any] | None = _load_intrinsic("molt_stream_new")
_molt_stream_recv: Callable[[Any], Any] | None = _load_intrinsic("molt_stream_recv")
_molt_stream_send_obj: Callable[[Any, Any], int] | None = _load_intrinsic(
    "molt_stream_send_obj"
)
_molt_stream_close: Callable[[Any], None] | None = _load_intrinsic("molt_stream_close")
_molt_stream_drop: Callable[[Any], None] | None = _load_intrinsic("molt_stream_drop")
if _molt_stream_new is None:

    def _molt_stream_new(capacity: int) -> Any:
        return molt_stream_new(capacity)  # type: ignore[name-defined]  # noqa: F821


if _molt_stream_recv is None:

    def _molt_stream_recv(stream: Any) -> Any:
        return molt_stream_recv(stream)  # type: ignore[name-defined]  # noqa: F821


if _molt_stream_send_obj is None:

    def _molt_stream_send_obj(stream: Any, data: Any) -> int:
        return molt_stream_send_obj(stream, data)  # type: ignore[name-defined]  # noqa: F821


if _molt_stream_close is None:

    def _molt_stream_close(stream: Any) -> None:
        molt_stream_close(stream)  # type: ignore[name-defined]  # noqa: F821
        return None


if _molt_stream_drop is None:

    def _molt_stream_drop(stream: Any) -> None:
        molt_stream_drop(stream)  # type: ignore[name-defined]  # noqa: F821
        return None


def _debug_exc_state(tag: str) -> None:
    if not _DEBUG_ASYNCIO_EXC:
        return None
    try:
        pending = (
            _molt_exception_pending() if _molt_exception_pending is not None else 0
        )
        last_obj = (
            _molt_exception_last()
            if pending and _molt_exception_last is not None
            else None
        )
        last_type = type(last_obj).__name__ if last_obj is not None else "None"
        _debug_write(
            "asyncio_exc tag={tag} pending={pending} last={last}".format(
                tag=tag, pending=int(bool(pending)), last=last_type
            )
        )
    except Exception:
        return None
    return None


class _SubprocessConstants:
    PIPE = _PROC_STDIO_PIPE
    DEVNULL = _PROC_STDIO_DEVNULL
    STDOUT = -2


subprocess = _SubprocessConstants


def _fd_from_fileobj(fileobj: Any) -> int:
    if isinstance(fileobj, int):
        return fileobj
    if hasattr(fileobj, "fileno"):
        return int(fileobj.fileno())
    raise ValueError("fileobj must be a file descriptor or have fileno()")


def _normalize_proc_stdio(value: Any, *, allow_stdout: bool) -> int:
    if value is None:
        return _PROC_STDIO_INHERIT
    if value is subprocess.PIPE:
        return _PROC_STDIO_PIPE
    if value is subprocess.DEVNULL:
        return _PROC_STDIO_DEVNULL
    if allow_stdout and value is subprocess.STDOUT:
        return subprocess.STDOUT
    # TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:missing): support subprocess stdio redirection via file descriptors and asyncio.subprocess.STDOUT.
    raise NotImplementedError("unsupported subprocess stdio option")


class _NonBlockingSocket:
    def __init__(self, sock: Any) -> None:
        self._sock = sock
        self._prev: float | None | object = _UNSET

    def __enter__(self) -> None:
        if not hasattr(self._sock, "gettimeout"):
            return None
        try:
            prev = self._sock.gettimeout()
        except Exception:
            return None
        self._prev = prev
        try:
            self._sock.settimeout(0.0)
        except Exception:
            return None
        return None

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        if self._prev is _UNSET:
            return None
        try:
            self._sock.settimeout(self._prev)
        except Exception:
            return None


def _swap_current_token(token: CancellationToken) -> int:
    try:
        return molt_cancel_token_set_current(token.token_id())  # type: ignore[name-defined]
    except Exception:
        return 0


def _restore_token_id(token_id: int) -> None:
    try:
        molt_cancel_token_set_current(token_id)  # type: ignore[name-defined]
    except Exception:
        return None


def _current_token_id() -> int:
    try:
        return molt_cancel_token_get_current()  # type: ignore[name-defined]
    except Exception:
        return 0


def _debug_write(message: str) -> None:
    err = getattr(_sys, "stderr", None)
    if err is None or not hasattr(err, "write"):
        err = getattr(_sys, "__stderr__", None)
    if err is not None and hasattr(err, "write"):
        try:
            err.write(f"{message}\n")
            err.flush()
            return None
        except Exception:
            pass
    out = getattr(_sys, "stdout", None)
    if out is not None and hasattr(out, "write"):
        try:
            out.write(f"{message}\n")
            out.flush()
            return None
        except Exception:
            pass
    try:
        print(message)
    except Exception:
        return None


def _future_done(task: Any) -> bool:
    if isinstance(task, Future):
        return task._done
    try:
        return task.done()
    except Exception:
        return False


def _future_cancelled(task: Any) -> bool:
    if isinstance(task, Future):
        return task._cancelled
    try:
        return task.cancelled()
    except Exception:
        return False


def _future_exception(task: Any) -> BaseException | None:
    if isinstance(task, Future):
        return task._exception
    try:
        return task.exception()
    except BaseException as err:
        return err


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


_TASK_COUNTER = 0


def _next_task_name() -> str:
    global _TASK_COUNTER
    _TASK_COUNTER += 1
    return f"Task-{_TASK_COUNTER}"


class Task(Future):
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
        try:
            task_dict = getattr(self, "__dict__", None)
            if isinstance(task_dict, dict):
                task_dict["_coro"] = coro
        except Exception:
            pass
        self._runner_task: Any | None = None
        self._token = CancellationToken()
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
        _TASKS[self._token.token_id()] = self
        self._runner_spawned = _spawn_runner
        token_id = self._token.token_id()
        try:
            molt_task_register_token_owned(self._coro, token_id)  # type: ignore[name-defined]
        except Exception:
            pass
        if _spawn_runner:
            prev_id = _swap_current_token(self._token)
            try:
                runner = self._runner(self._coro)
                self._runner_task = runner
                try:
                    molt_task_register_token_owned(  # type: ignore[name-defined]
                        runner, token_id
                    )
                except Exception:
                    pass
                spawn(runner)
            finally:
                _restore_token_id(prev_id)

    def _rebind_token(self, token: CancellationToken) -> None:
        old_token = self._token
        old_id = old_token.token_id()
        new_id = token.token_id()
        if new_id == old_id:
            return
        if _TASKS.get(old_id) is self:
            _TASKS.pop(old_id, None)
        self._token = token
        _TASKS[new_id] = self
        try:
            _contextvars._set_context_for_token(  # type: ignore[unresolved-attribute]
                new_id, self._context
            )
        except Exception:
            pass
        try:
            _contextvars._clear_context_for_token(  # type: ignore[unresolved-attribute]
                old_id
            )
        except Exception:
            pass

    def cancel(self, msg: Any | None = None) -> bool:
        if self._done:
            return False
        self._cancel_requested += 1
        if msg is None:
            self._cancel_message = None
        else:
            self._cancel_message = msg
        if _DEBUG_TASKS:
            token_id = self._token.token_id()
            _debug_write(
                "asyncio_task_cancel token={token} msg={msg!r}".format(
                    token=token_id, msg=msg
                )
            )
        self._token.cancel()
        try:
            if msg is not None and _molt_future_cancel_msg is not None:
                _molt_future_cancel_msg(self._coro, msg)
            else:
                molt_future_cancel(self._coro)  # type: ignore[name-defined]
        except Exception:
            pass
        return True

    def get_coro(self) -> Any:
        return self._coro

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
            try:
                if _molt_future_cancel_clear is not None:
                    _molt_future_cancel_clear(self._coro)
            except Exception:
                pass
        return self._cancel_requested

    async def _runner(self, coro: Any | None = None) -> None:
        result: Any = None
        exc: BaseException | None = None
        extra_token_id: int | None = None
        if coro is None:
            coro = getattr(self, "_coro")
        current_id = _current_token_id()
        if current_id != self._token.token_id() and current_id not in _TASKS:
            _TASKS[current_id] = self
            extra_token_id = current_id
        if _DEBUG_TASKS:
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
            if _DEBUG_TASKS:
                token_id = self._token.token_id()
                _debug_write(
                    "asyncio_task_exc token={token_id} type={exc_type}".format(
                        token_id=token_id,
                        exc_type=type(err).__name__,
                    )
                )
        if exc is None:
            if not self._done:
                self.set_result(result)
                if _DEBUG_TASKS:
                    token_id = self._token.token_id()
                    _debug_write(f"asyncio_task_done token={token_id}")
        else:
            if not self._done:
                self.set_exception(exc)
        _cleanup_event_waiters_for_token(self._token.token_id())
        _TASKS.pop(self._token.token_id(), None)
        if extra_token_id is not None:
            _TASKS.pop(extra_token_id, None)
        _contextvars._clear_context_for_token(  # type: ignore[unresolved-attribute]
            self._token.token_id()
        )

    def __repr__(self) -> str:
        if self._cancelled:
            state = "cancelled"
        elif self._done:
            state = "finished"
        else:
            state = "pending"
        return f"<Task {self._name} {state}>"

    def __await__(self) -> Any:
        if self._done:
            return self._wait().__await__()
        waiter = Future()

        def _transfer(done: Future) -> None:
            if waiter.done():
                return
            try:
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
        token_id = _current_token_id()
        fut._molt_event_token_id = token_id
        self._waiters.append(fut)
        _register_event_waiter(token_id, fut)
        try:
            return await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                _unregister_event_waiter(token_id, fut)
                idx = 0
                while idx < len(self._waiters):
                    if self._waiters[idx] is fut:
                        del self._waiters[idx]
                        break
                    idx += 1
            raise


class Lock:
    def __init__(self) -> None:
        self._locked = False
        self._waiters: _deque[Future] = _deque()

    def locked(self) -> bool:
        return self._locked

    async def acquire(self) -> bool:
        if not self._locked:
            self._locked = True
            return True
        fut = Future()
        self._waiters.append(fut)
        try:
            await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                try:
                    self._waiters.remove(fut)
                except ValueError:
                    pass
            raise
        self._locked = True
        return True

    def release(self) -> None:
        if not self._locked:
            raise RuntimeError("Lock is not acquired")
        if self._waiters:
            fut = self._waiters.popleft()
            if not fut.done():
                fut.set_result(True)
        else:
            self._locked = False

    async def __aenter__(self) -> "Lock":
        await self.acquire()
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.release()


class Condition:
    def __init__(self, lock: Lock | None = None) -> None:
        self._lock = lock or Lock()
        self._waiters: _deque[Future] = _deque()

    def locked(self) -> bool:
        return self._lock.locked()

    async def acquire(self) -> bool:
        return await self._lock.acquire()

    def release(self) -> None:
        self._lock.release()

    async def __aenter__(self) -> "Condition":
        await self.acquire()
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.release()

    async def wait(self) -> bool:
        if not self.locked():
            raise RuntimeError("Condition lock is not acquired")
        fut = Future()
        self._waiters.append(fut)
        self.release()
        try:
            await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                try:
                    self._waiters.remove(fut)
                except ValueError:
                    pass
            raise
        finally:
            await self.acquire()
        return True

    async def wait_for(self, predicate: Callable[[], bool]) -> bool:
        result = predicate()
        while not result:
            await self.wait()
            result = predicate()
        return result

    def notify(self, n: int = 1) -> None:
        if not self.locked():
            raise RuntimeError("Condition lock is not acquired")
        count = min(n, len(self._waiters))
        for _ in range(count):
            fut = self._waiters.popleft()
            if not fut.done():
                fut.set_result(True)

    def notify_all(self) -> None:
        self.notify(len(self._waiters))


class Semaphore:
    def __init__(self, value: int = 1) -> None:
        if value < 0:
            raise ValueError("Semaphore initial value must be >= 0")
        self._value = value
        self._waiters: _deque[Future] = _deque()

    def locked(self) -> bool:
        return self._value == 0

    async def acquire(self) -> bool:
        if self._value > 0:
            self._value -= 1
            return True
        fut = Future()
        self._waiters.append(fut)
        try:
            await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                try:
                    self._waiters.remove(fut)
                except ValueError:
                    pass
            raise
        return True

    def release(self) -> None:
        if self._waiters:
            fut = self._waiters.popleft()
            if not fut.done():
                fut.set_result(True)
        else:
            self._value += 1


class BoundedSemaphore(Semaphore):
    def __init__(self, value: int = 1) -> None:
        super().__init__(value)
        self._initial_value = value

    def release(self) -> None:
        if not self._waiters and self._value >= self._initial_value:
            raise ValueError("BoundedSemaphore released too many times")
        super().release()


class Barrier:
    def __init__(self, parties: int) -> None:
        if parties <= 0:
            raise ValueError("Barrier parties must be > 0")
        self._parties = parties
        self._count = 0
        self._waiters: list[Future] = []
        self._broken = False

    async def wait(self) -> int:
        if self._broken:
            raise BrokenBarrierError("Barrier broken")
        fut = Future()
        self._waiters.append(fut)
        self._count += 1
        if self._count == self._parties:
            waiters = self._waiters
            self._waiters = []
            self._count = 0
            for idx, waiter in enumerate(waiters):
                if not waiter.done():
                    waiter.set_result(idx)
        try:
            return await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                try:
                    self._waiters.remove(fut)
                except ValueError:
                    pass
            raise

    @property
    def broken(self) -> bool:
        return self._broken

    def reset(self) -> None:
        self._broken = False
        self._count = 0
        self._waiters = []


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
        if isinstance(task, Task):
            if task not in self._tasks:
                return
            self._tasks.discard(task)
        try:
            exc = task.exception()
        except CancelledError:
            return
        except BaseException as err:
            self._errors.append(err)
            self._collect_done_errors()
            self._request_cancel()
            return
        if exc is not None and not _is_cancelled_exc(exc):
            self._errors.append(exc)
            self._collect_done_errors()
            self._request_cancel()

    def _collect_done_errors(self) -> None:
        for task in list(self._tasks):
            if not task.done():
                continue
            self._tasks.discard(task)
            try:
                exc = task.exception()
            except CancelledError:
                continue
            except BaseException as err:
                self._errors.append(err)
                continue
            if exc is not None and not _is_cancelled_exc(exc):
                self._errors.append(exc)

    def _request_cancel(self) -> None:
        loop = self._loop
        if loop is None:
            self._cancel_all()
            return
        if self._cancel_handle is not None:
            return
        self._cancel_handle = loop.call_soon(self._cancel_all)

    async def _wait_tasks(self) -> None:
        for task in list(self._tasks):
            try:
                await task
            except BaseException:
                pass

    def _cancel_all(self) -> None:
        self._cancel_handle = None
        for task in list(self._tasks):
            if not task.done():
                task.cancel()


class _Timeout:
    def __init__(self, when: float | None) -> None:
        self._when = when
        self._loop: EventLoop | None = None
        self._task: Task | None = None
        self._handle: TimerHandle | None = None
        self._timed_out = False

    def _on_timeout(self) -> None:
        if self._task is None or self._timed_out:
            return
        self._timed_out = True
        try:
            self._task.cancel()
        except Exception:
            pass

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
            try:
                self._handle.cancel()
            except Exception:
                pass
        if exc is None:
            return False
        if self._timed_out and _is_cancelled_exc(exc):
            if self._task is not None:
                try:
                    self._task.uncancel()
                except Exception:
                    pass
            raise TimeoutError
        return False


class IncompleteReadError(EOFError):
    def __init__(self, partial: bytes, expected: int) -> None:
        super().__init__(f"{expected} bytes expected, {len(partial)} bytes read")
        self.partial = partial
        self.expected = expected


class StreamReader:
    def __init__(self, sock: _socket.socket) -> None:
        self._sock = sock
        self._buffer = bytearray()
        self._eof = False
        try:
            self._sock.setblocking(False)
        except Exception:
            pass
        try:
            self._fd = self._sock.fileno()
        except Exception:
            self._fd = -1

    async def _wait_readable(self) -> None:
        if _molt_io_wait_new is None:
            # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio streams readiness.
            raise NotImplementedError("I/O polling unavailable")
        await _io_wait(self._fd, 1)

    async def _recv(self, size: int) -> bytes:
        while True:
            try:
                return self._sock.recv(size)
            except (BlockingIOError, InterruptedError):
                await self._wait_readable()
            except OSError as exc:
                if exc.errno in (_errno.EAGAIN, _errno.EWOULDBLOCK):
                    await self._wait_readable()
                    continue
                raise

    def at_eof(self) -> bool:
        return self._eof and not self._buffer

    async def read(self, n: int = -1) -> bytes:
        if n == 0:
            return b""
        if n < 0:
            chunks: list[bytes] = []
            if self._buffer:
                chunks.append(bytes(self._buffer))
                self._buffer.clear()
            while not self._eof:
                data = await self._recv(4096)
                if not data:
                    self._eof = True
                    break
                chunks.append(data)
            return b"".join(chunks)
        while len(self._buffer) < n and not self._eof:
            data = await self._recv(n - len(self._buffer))
            if not data:
                self._eof = True
                break
            self._buffer.extend(data)
        if not self._buffer:
            return b""
        out = bytes(self._buffer[:n])
        del self._buffer[:n]
        return out

    async def readexactly(self, n: int) -> bytes:
        if n <= 0:
            return b""
        while len(self._buffer) < n and not self._eof:
            data = await self._recv(n - len(self._buffer))
            if not data:
                self._eof = True
                break
            self._buffer.extend(data)
        if len(self._buffer) < n:
            partial = bytes(self._buffer)
            self._buffer.clear()
            raise IncompleteReadError(partial, n)
        out = bytes(self._buffer[:n])
        del self._buffer[:n]
        return out


class StreamWriter:
    def __init__(self, sock: _socket.socket) -> None:
        self._sock = sock
        self._buffer = bytearray()
        self._closed = False
        try:
            self._sock.setblocking(False)
        except Exception:
            pass
        try:
            self._fd = self._sock.fileno()
        except Exception:
            self._fd = -1

    def write(self, data: bytes) -> None:
        if self._closed:
            return None
        if not isinstance(data, (bytes, bytearray, memoryview)):
            raise TypeError("data must be bytes-like")
        self._buffer.extend(data)
        return None

    async def drain(self) -> None:
        if _molt_io_wait_new is None:
            # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio stream writes.
            raise NotImplementedError("I/O polling unavailable")
        while self._buffer:
            try:
                sent = self._sock.send(self._buffer)
                if sent <= 0:
                    await _io_wait(self._fd, 2)
                    continue
                del self._buffer[:sent]
            except (BlockingIOError, InterruptedError):
                await _io_wait(self._fd, 2)
            except OSError as exc:
                if exc.errno in (_errno.EAGAIN, _errno.EWOULDBLOCK):
                    await _io_wait(self._fd, 2)
                    continue
                raise

    def write_eof(self) -> None:
        try:
            self._sock.shutdown(_socket.SHUT_WR)
        except Exception:
            pass

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        try:
            self._sock.close()
        except Exception:
            pass

    async def wait_closed(self) -> None:
        return None


class AbstractServer:
    def close(self) -> None:
        raise NotImplementedError

    async def wait_closed(self) -> None:
        raise NotImplementedError

    def is_serving(self) -> bool:
        raise NotImplementedError

    async def start_serving(self) -> None:
        raise NotImplementedError

    async def serve_forever(self) -> None:
        raise NotImplementedError

    def get_loop(self) -> "EventLoop":
        raise NotImplementedError

    def close_clients(self) -> None:
        raise NotImplementedError

    def abort_clients(self) -> None:
        raise NotImplementedError


class Server(AbstractServer):
    def __init__(self, sock: _socket.socket, callback: Any) -> None:
        self._sock = sock
        self._callback = callback
        self.sockets = [sock]
        self._closed = False
        self._accept_task = get_running_loop().create_task(
            self._accept_loop(), name=None, context=None
        )

    async def _accept_loop(self) -> None:
        while not self._closed:
            try:
                conn, _addr = self._sock.accept()
            except (BlockingIOError, InterruptedError):
                if _molt_io_wait_new is None:
                    # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio server accept.
                    raise NotImplementedError("I/O polling unavailable")
                await _io_wait(self._sock.fileno(), 1)
                continue
            except OSError as exc:
                if exc.errno in (_errno.EAGAIN, _errno.EWOULDBLOCK):
                    if _molt_io_wait_new is None:
                        # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio server accept.
                        raise NotImplementedError("I/O polling unavailable")
                    await _io_wait(self._sock.fileno(), 1)
                    continue
                raise
            except BaseException as exc:
                if _is_cancelled_exc(exc):
                    break
                if self._closed:
                    break
                continue
            try:
                conn.setblocking(False)
            except Exception:
                pass
            reader = StreamReader(conn)
            writer = StreamWriter(conn)
            try:
                get_running_loop().create_task(
                    self._callback(reader, writer), name=None, context=None
                )
            except Exception:
                try:
                    conn.close()
                except Exception:
                    pass

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        try:
            self._sock.close()
        except Exception:
            pass
        try:
            self._accept_task.cancel()
        except Exception:
            pass

    async def wait_closed(self) -> None:
        try:
            await self._accept_task
        except BaseException:
            return None


class ProcessStreamReader:
    def __init__(self, handle: Any) -> None:
        self._handle = handle
        self._buffer = bytearray()
        self._eof = False

    async def _recv_chunk(self) -> bytes:
        while True:
            res = _require_asyncio_intrinsic(_molt_stream_recv, "stream_recv")(
                self._handle
            )
            if res == _PENDING:
                await sleep(0.0)
                continue
            if res is not None and not isinstance(res, (bytes, bytearray, memoryview)):
                await sleep(0.0)
                continue
            if res is None:
                self._eof = True
                return b""
            if isinstance(res, (bytes, bytearray, memoryview)):
                return bytes(res)
            raise TypeError("process stream recv returned non-bytes")

    def at_eof(self) -> bool:
        return self._eof and not self._buffer

    async def read(self, n: int = -1) -> bytes:
        if n == 0:
            return b""
        if n < 0:
            buf = bytearray()
            if self._buffer:
                buf += bytes(self._buffer)
                self._buffer.clear()
            while not self._eof:
                data = await self._recv_chunk()
                if not data:
                    break
                buf += data
            return bytes(buf)
        while len(self._buffer) < n and not self._eof:
            data = await self._recv_chunk()
            if not data:
                break
            self._buffer.extend(data)
        if not self._buffer:
            return b""
        out = bytes(self._buffer[:n])
        del self._buffer[:n]
        return out

    async def readline(self) -> bytes:
        while True:
            if self._eof and not self._buffer:
                return b""
            idx = self._buffer.find(b"\n")
            if idx != -1:
                idx += 1
                out = bytes(self._buffer[:idx])
                del self._buffer[:idx]
                return out
            data = await self._recv_chunk()
            if not data:
                if not self._buffer:
                    return b""
                out = bytes(self._buffer)
                self._buffer.clear()
                return out
            self._buffer.extend(data)

    async def readexactly(self, n: int) -> bytes:
        if n <= 0:
            return b""
        while len(self._buffer) < n and not self._eof:
            data = await self._recv_chunk()
            if not data:
                break
            self._buffer.extend(data)
        if len(self._buffer) < n:
            partial = bytes(self._buffer)
            self._buffer.clear()
            raise IncompleteReadError(partial, n)
        out = bytes(self._buffer[:n])
        del self._buffer[:n]
        return out


class ProcessStreamWriter:
    def __init__(self, handle: Any) -> None:
        self._handle = handle
        self._buffer = bytearray()
        self._closed = False

    def write(self, data: bytes) -> None:
        if self._closed:
            return None
        if not isinstance(data, (bytes, bytearray, memoryview)):
            raise TypeError("data must be bytes-like")
        self._buffer.extend(data)
        return None

    async def drain(self) -> None:
        while self._buffer:
            res = _require_asyncio_intrinsic(_molt_stream_send_obj, "stream_send_obj")(
                self._handle, bytes(self._buffer)
            )
            if res == _PENDING or not isinstance(res, int) or res != 0:
                await sleep(0.0)
                continue
            self._buffer.clear()

    def write_eof(self) -> None:
        self.close()

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        try:
            _require_asyncio_intrinsic(_molt_stream_close, "stream_close")(self._handle)
        except Exception:
            pass

    async def wait_closed(self) -> None:
        return None


class Process:
    def __init__(
        self,
        handle: Any,
        stdin: ProcessStreamWriter | None,
        stdout: ProcessStreamReader | None,
        stderr: ProcessStreamReader | None,
    ) -> None:
        self._handle = handle
        self.stdin = stdin
        self.stdout = stdout
        self.stderr = stderr
        self._wait_future: Any | None = None

    @property
    def pid(self) -> int:
        return int(
            _require_asyncio_intrinsic(_molt_process_pid, "process_pid")(self._handle)
        )

    @property
    def returncode(self) -> int | None:
        return _require_asyncio_intrinsic(
            _molt_process_returncode, "process_returncode"
        )(self._handle)

    def kill(self) -> None:
        _require_asyncio_intrinsic(_molt_process_kill, "process_kill")(self._handle)

    def terminate(self) -> None:
        _require_asyncio_intrinsic(_molt_process_terminate, "process_terminate")(
            self._handle
        )

    async def wait(self) -> int:
        wait_future = getattr(self, "_wait_future", None)
        if wait_future is None:
            fut = _require_asyncio_intrinsic(
                _molt_process_wait_future, "process_wait_future"
            )(self._handle)
            self._wait_future = fut
            wait_future = fut
        code = int(await wait_future)
        watcher = _CHILD_WATCHER
        if watcher is not None and hasattr(watcher, "_notify_child_exit"):
            try:
                watcher._notify_child_exit(self.pid, code)
            except Exception:
                pass
        return code

    async def communicate(self, input: bytes | None = None) -> tuple[bytes, bytes]:
        if input is not None:
            if self.stdin is None:
                raise ValueError("stdin was not set to PIPE")
            self.stdin.write(input)
            await self.stdin.drain()
            self.stdin.close()

        tasks: list[Future] = []
        if self.stdout is not None:
            tasks.append(ensure_future(self.stdout.read()))
        if self.stderr is not None:
            tasks.append(ensure_future(self.stderr.read()))

        out: bytes | None = None
        err: bytes | None = None
        try:
            if tasks:
                results = await gather(*tasks)
                if self.stdout is not None:
                    out = results[0]
                if self.stderr is not None:
                    err = results[-1]
            await self.wait()
        except BaseException:
            for task in tasks:
                try:
                    task.cancel()
                except Exception:
                    pass
            raise
        return out or b"", err or b""

    def __del__(self) -> None:
        try:
            _require_asyncio_intrinsic(_molt_process_drop, "process_drop")(self._handle)
        except Exception:
            pass


class Handle:
    def __init__(
        self,
        callback: Callable[..., Any],
        args: tuple[Any, ...],
        loop: "EventLoop",
        context: Any | None,
    ) -> None:
        self._callback = callback
        self._args = args
        self._loop = loop
        self._context = context
        self._cancelled = False

    def cancel(self) -> None:
        self._cancelled = True

    def cancelled(self) -> bool:
        return self._cancelled

    def _run(self) -> None:
        if self._cancelled:
            return
        try:
            if self._context is not None:
                self._context.run(self._callback, *self._args)
            else:
                self._callback(*self._args)
        except BaseException as exc:
            try:
                self._loop.call_exception_handler(
                    {
                        "message": "Unhandled exception in callback",
                        "exception": exc,
                        "handle": self,
                    }
                )
            except Exception:
                pass


class TimerHandle(Handle):
    def __init__(
        self,
        when: float,
        callback: Callable[..., Any],
        args: tuple[Any, ...],
        loop: "EventLoop",
        context: Any | None,
    ) -> None:
        super().__init__(callback, args, loop, context)
        self._when = when
        self._timer_task: Task | None = None

    def when(self) -> float:
        return self._when

    def cancel(self) -> None:
        super().cancel()
        if self._timer_task is not None:
            try:
                self._timer_task.cancel()
            except Exception:
                pass
        try:
            self._loop._scheduled.discard(self)  # type: ignore[attr-defined]
        except Exception:
            pass


class AbstractEventLoop:
    def run_forever(self) -> None:
        raise NotImplementedError

    def run_until_complete(self, future: Any) -> Any:
        raise NotImplementedError

    def stop(self) -> None:
        raise NotImplementedError

    def is_running(self) -> bool:
        raise NotImplementedError

    def is_closed(self) -> bool:
        raise NotImplementedError

    def close(self) -> None:
        raise NotImplementedError

    async def shutdown_asyncgens(self) -> None:
        raise NotImplementedError

    async def shutdown_default_executor(self) -> None:
        raise NotImplementedError

    def create_task(
        self, coro: Any, *, name: str | None = None, context: Any | None = None
    ) -> Task:
        raise NotImplementedError

    def set_task_factory(self, factory: Callable[..., Task] | None) -> None:
        raise NotImplementedError

    def get_task_factory(self) -> Callable[..., Task] | None:
        raise NotImplementedError

    def create_future(self) -> Future:
        raise NotImplementedError

    def call_soon(
        self, callback: Callable[..., Any], /, *args: Any, context: Any | None = None
    ) -> Handle:
        raise NotImplementedError

    def call_soon_threadsafe(
        self, callback: Callable[..., Any], /, *args: Any, context: Any | None = None
    ) -> Handle:
        raise NotImplementedError

    def call_later(
        self,
        delay: float,
        callback: Callable[..., Any],
        /,
        *args: Any,
        context: Any | None = None,
    ) -> TimerHandle:
        raise NotImplementedError

    def call_at(
        self,
        when: float,
        callback: Callable[..., Any],
        /,
        *args: Any,
        context: Any | None = None,
    ) -> TimerHandle:
        raise NotImplementedError

    def time(self) -> float:
        raise NotImplementedError

    def call_exception_handler(self, context: dict[str, Any]) -> None:
        raise NotImplementedError

    def default_exception_handler(self, context: dict[str, Any]) -> None:
        raise NotImplementedError

    def set_exception_handler(
        self, handler: Callable[["AbstractEventLoop", dict[str, Any]], Any] | None
    ) -> None:
        raise NotImplementedError

    def get_exception_handler(
        self,
    ) -> Callable[["AbstractEventLoop", dict[str, Any]], Any] | None:
        raise NotImplementedError

    def get_debug(self) -> bool:
        raise NotImplementedError

    def set_debug(self, enabled: bool) -> None:
        raise NotImplementedError

    def add_signal_handler(
        self, sig: int, callback: Callable[..., Any], /, *args: Any
    ) -> None:
        raise NotImplementedError

    def remove_signal_handler(self, sig: int) -> bool:
        raise NotImplementedError

    def add_reader(self, fd: int, callback: Callable[..., Any], /, *args: Any) -> None:
        raise NotImplementedError

    def remove_reader(self, fd: int) -> bool:
        raise NotImplementedError

    def add_writer(self, fd: int, callback: Callable[..., Any], /, *args: Any) -> None:
        raise NotImplementedError

    def remove_writer(self, fd: int) -> bool:
        raise NotImplementedError

    async def create_connection(
        self,
        protocol_factory: Callable[[], Protocol] | None,
        host: str | None = None,
        port: int | None = None,
        /,
        **kwargs: Any,
    ) -> tuple[Transport, Protocol]:
        raise NotImplementedError

    async def create_server(
        self,
        protocol_factory: Callable[[], Protocol],
        host: str | None = None,
        port: int | None = None,
        /,
        **kwargs: Any,
    ) -> AbstractServer:
        raise NotImplementedError

    async def create_datagram_endpoint(
        self,
        protocol_factory: Callable[[], DatagramProtocol],
        local_addr: Any | None = None,
        remote_addr: Any | None = None,
        /,
        **kwargs: Any,
    ) -> tuple[DatagramTransport, DatagramProtocol]:
        raise NotImplementedError

    async def connect_accepted_socket(
        self,
        protocol_factory: Callable[[], Protocol],
        sock: _socket.socket,
        /,
        **kwargs,
    ) -> tuple[Transport, Protocol]:
        raise NotImplementedError

    async def create_unix_connection(
        self,
        protocol_factory: Callable[[], Protocol],
        path: str | None = None,
        /,
        **kwargs: Any,
    ) -> tuple[Transport, Protocol]:
        raise NotImplementedError

    async def create_unix_server(
        self,
        protocol_factory: Callable[[], Protocol],
        path: str | None = None,
        /,
        **kwargs: Any,
    ) -> AbstractServer:
        raise NotImplementedError

    async def create_subprocess_shell(self, protocol_factory: Any, cmd: Any, **kwargs):
        raise NotImplementedError

    async def create_subprocess_exec(self, protocol_factory: Any, *args: Any, **kwargs):
        raise NotImplementedError

    async def start_tls(
        self, transport: Transport, protocol: Protocol, sslcontext: Any
    ):
        raise NotImplementedError

    async def sendfile(self, transport: Transport, file: Any, **kwargs: Any) -> Any:
        raise NotImplementedError

    def set_default_executor(self, executor: Any) -> None:
        raise NotImplementedError

    def run_in_executor(self, executor: Any, func: Any, *args: Any) -> Future:
        raise NotImplementedError

    async def getaddrinfo(self, host: Any, port: Any, **kwargs: Any) -> Any:
        raise NotImplementedError

    async def getnameinfo(self, sockaddr: Any, flags: int) -> Any:
        raise NotImplementedError

    async def sock_recv(self, sock: Any, n: int) -> bytes:
        raise NotImplementedError

    async def sock_recv_into(self, sock: Any, buf: Any) -> int:
        raise NotImplementedError

    async def sock_recvfrom(self, sock: Any, bufsize: int) -> tuple[Any, Any]:
        raise NotImplementedError

    async def sock_recvfrom_into(self, sock: Any, buf: Any) -> tuple[int, Any]:
        raise NotImplementedError

    async def sock_sendall(self, sock: Any, data: bytes) -> None:
        raise NotImplementedError

    async def sock_sendto(self, sock: Any, data: bytes, addr: Any) -> int:
        raise NotImplementedError

    async def sock_connect(self, sock: Any, address: Any) -> None:
        raise NotImplementedError

    async def sock_accept(self, sock: Any) -> tuple[Any, Any]:
        raise NotImplementedError

    async def sock_sendfile(self, sock: Any, file: Any, offset: int = 0, count=None):
        raise NotImplementedError


class _EventLoop(AbstractEventLoop):
    def __init__(self, selector: Any | None = None) -> None:
        self._readers: dict[int, tuple[Any, tuple[Any, ...], Task]] = {}
        self._writers: dict[int, tuple[Any, tuple[Any, ...], Task]] = {}
        self._ready: _deque[Handle] = _deque()
        self._scheduled: set[TimerHandle] = set()
        self._ready_task: Task | None = None
        self._closed = False
        self._running = False
        self._stopping = False
        self._exception_handler: Callable[["EventLoop", dict[str, Any]], Any] | None = (
            None
        )
        self._debug = False
        self._task_factory: Callable[..., Task] | None = None
        self._selector = selector
        self._time_origin = _time.monotonic()

    def create_future(self) -> Future:
        return Future()

    def create_task(
        self, coro: Any, *, name: str | None = None, context: Any | None = None
    ) -> Task:
        if self._task_factory is None:
            return Task(coro, loop=self, name=name, context=context)
        if context is None:
            task = self._task_factory(self, coro)
        else:
            task = self._task_factory(self, coro, context=context)
        if name is not None:
            try:
                setter = getattr(task, "set_name", None)
                if callable(setter):
                    setter(name)
                else:
                    setattr(task, "_name", name)
            except Exception:
                pass
        return task

    def _ensure_ready_runner(self) -> None:
        if self._ready_task is not None and not self._ready_task.done():
            return
        self._ready_task = self.create_task(self._ready_loop(), name=None, context=None)

    async def _ready_loop(self) -> None:
        while not self._closed:
            while self._ready:
                handle = self._ready.popleft()
                if handle.cancelled():
                    continue
                handle._run()
            try:
                await sleep(0.0)
            except BaseException as exc:
                if _is_cancelled_exc(exc):
                    return
                raise

    def call_soon(
        self, callback: Callable[..., Any], /, *args: Any, context: Any | None = None
    ) -> Handle:
        if self._closed:
            raise RuntimeError("Event loop is closed")
        if context is None:
            try:
                context = _contextvars.copy_context()
            except Exception:
                context = None
        handle = Handle(callback, args, self, context)
        self._ready.append(handle)
        if self._running:
            self._ensure_ready_runner()
        return handle

    def call_soon_threadsafe(
        self, callback: Callable[..., Any], /, *args: Any, context: Any | None = None
    ) -> Handle:
        return self.call_soon(callback, *args, context=context)

    def call_later(
        self,
        delay: float,
        callback: Callable[..., Any],
        /,
        *args: Any,
        context: Any | None = None,
    ) -> TimerHandle:
        if _DEBUG_ASYNCIO_EXC:
            time_attr = getattr(type(self), "time", None)
            time_owner = getattr(time_attr, "__qualname__", repr(time_attr))
            _debug_write(
                f"call_later loop={type(self).__name__} time={time_owner} delay={delay}"
            )
        if delay <= 0:
            return self.call_at(self.time(), callback, *args, context=context)
        when = self.time() + float(delay)
        handle = TimerHandle(when, callback, args, self, context)
        self._scheduled.add(handle)

        async def _timer() -> None:
            await molt_async_sleep(delay)
            if handle.cancelled():
                self._scheduled.discard(handle)
                return
            self._scheduled.discard(handle)
            self._ready.append(handle)
            if self._running:
                self._ensure_ready_runner()

        handle._timer_task = self.create_task(_timer(), name=None, context=None)
        return handle

    def call_at(
        self,
        when: float,
        callback: Callable[..., Any],
        /,
        *args: Any,
        context: Any | None = None,
    ) -> TimerHandle:
        delay = max(0.0, float(when) - self.time())
        handle = TimerHandle(float(when), callback, args, self, context)
        if delay <= 0:
            self._ready.append(handle)
            if self._running:
                self._ensure_ready_runner()
            return handle
        self._scheduled.add(handle)

        async def _timer() -> None:
            await molt_async_sleep(delay)
            if handle.cancelled():
                self._scheduled.discard(handle)
                return
            self._scheduled.discard(handle)
            self._ready.append(handle)
            if self._running:
                self._ensure_ready_runner()

        handle._timer_task = self.create_task(_timer(), name=None, context=None)
        return handle

    def set_exception_handler(
        self, handler: Callable[["EventLoop", dict[str, Any]], Any] | None
    ) -> None:
        self._exception_handler = handler

    def call_exception_handler(self, context: dict[str, Any]) -> None:
        handler = self._exception_handler
        if handler is not None:
            handler(self, context)
            return
        message = context.get("message", "Unhandled exception in event loop")
        exc = context.get("exception")
        if exc is None:
            _debug_write(message)
        else:
            _debug_write(f"{message}: {exc}")

    def set_debug(self, enabled: bool) -> None:
        self._debug = bool(enabled)

    def get_debug(self) -> bool:
        return self._debug

    def set_task_factory(self, factory: Callable[..., Task] | None) -> None:
        self._task_factory = factory

    def get_task_factory(self) -> Callable[..., Task] | None:
        return self._task_factory

    def time(self) -> float:
        return _time.monotonic() - self._time_origin

    def is_running(self) -> bool:
        return self._running

    def is_closed(self) -> bool:
        return self._closed

    def stop(self) -> None:
        self._stopping = True

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        if self._ready_task is not None:
            try:
                self._ready_task.cancel()
            except Exception:
                pass
        if self._selector is not None:
            try:
                self._selector.close()
            except Exception:
                pass

    def run_in_executor(self, executor: Any, func: Any, *args: Any) -> Future:
        if executor is not None:
            # TODO(tooling, owner:tooling, milestone:TL2, priority:P1, status:missing): support custom executors in asyncio run_in_executor.
            raise NotImplementedError("custom executors not supported")
        future = molt_thread_submit(func, args, {})
        return future

    def add_reader(self, fd: Any, callback: Any, *args: Any) -> None:
        if _molt_io_wait_new is None:
            # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio add_reader.
            raise NotImplementedError("I/O polling unavailable")
        io_wait = _require_asyncio_intrinsic(_molt_io_wait_new, "io_wait_new")
        fileno = _fd_from_fileobj(fd)
        if fileno in self._readers:
            self.remove_reader(fileno)

        async def _reader_loop() -> None:
            while fileno in self._readers:
                try:
                    await io_wait(fileno, 1, None)
                except BaseException as exc:
                    if _is_cancelled_exc(exc):
                        break
                    return
                if fileno not in self._readers:
                    break
                try:
                    callback(*args)
                except Exception:
                    return

        task = self.create_task(_reader_loop(), name=None, context=None)
        self._readers[fileno] = (callback, args, task)

    def remove_reader(self, fd: Any) -> bool:
        fileno = _fd_from_fileobj(fd)
        entry = self._readers.pop(fileno, None)
        if entry is None:
            return False
        _callback, _args, task = entry
        try:
            task.cancel()
        except Exception:
            pass
        return True

    def add_writer(self, fd: Any, callback: Any, *args: Any) -> None:
        if _molt_io_wait_new is None:
            # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio add_writer.
            raise NotImplementedError("I/O polling unavailable")
        io_wait = _require_asyncio_intrinsic(_molt_io_wait_new, "io_wait_new")
        fileno = _fd_from_fileobj(fd)
        if fileno in self._writers:
            self.remove_writer(fileno)

        async def _writer_loop() -> None:
            while fileno in self._writers:
                try:
                    await io_wait(fileno, 2, None)
                except BaseException as exc:
                    if _is_cancelled_exc(exc):
                        break
                    return
                if fileno not in self._writers:
                    break
                try:
                    callback(*args)
                except Exception:
                    return

        task = self.create_task(_writer_loop(), name=None, context=None)
        self._writers[fileno] = (callback, args, task)

    async def sock_recv(self, sock: Any, n: int) -> bytes:
        if _molt_io_wait_new is None:
            # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio sock_recv.
            raise NotImplementedError("I/O polling unavailable")
        flags = getattr(_socket, "MSG_DONTWAIT", 0)
        while True:
            try:
                return sock.recv(n, flags)
            except (BlockingIOError, InterruptedError):
                await _io_wait(sock.fileno(), 1)
            except OSError as exc:
                if exc.errno in (_errno.EAGAIN, _errno.EWOULDBLOCK):
                    await _io_wait(sock.fileno(), 1)
                    continue
                raise

    async def sock_recv_into(self, sock: Any, buf: Any) -> int:
        if _molt_io_wait_new is None:
            # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio sock_recv_into.
            raise NotImplementedError("I/O polling unavailable")
        flags = getattr(_socket, "MSG_DONTWAIT", 0)
        nbytes = len(buf)
        while True:
            try:
                return sock.recv_into(buf, nbytes, flags)
            except (BlockingIOError, InterruptedError):
                await _io_wait(sock.fileno(), 1)
            except OSError as exc:
                if exc.errno in (_errno.EAGAIN, _errno.EWOULDBLOCK):
                    await _io_wait(sock.fileno(), 1)
                    continue
                raise

    async def sock_sendall(self, sock: Any, data: bytes) -> None:
        if _molt_io_wait_new is None:
            # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio sock_sendall.
            raise NotImplementedError("I/O polling unavailable")
        view = memoryview(data)
        total = 0
        flags = getattr(_socket, "MSG_DONTWAIT", 0)
        while total < len(view):
            try:
                sent = sock.send(view[total:], flags)
                if sent <= 0:
                    await _io_wait(sock.fileno(), 2)
                    continue
                total += sent
            except (BlockingIOError, InterruptedError):
                await _io_wait(sock.fileno(), 2)
            except OSError as exc:
                if exc.errno in (_errno.EAGAIN, _errno.EWOULDBLOCK):
                    await _io_wait(sock.fileno(), 2)
                    continue
                raise

    async def sock_connect(self, sock: Any, address: Any) -> None:
        if _molt_io_wait_new is None:
            # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio sock_connect.
            raise NotImplementedError("I/O polling unavailable")
        with _NonBlockingSocket(sock):
            err = sock.connect_ex(address)
        if err in (0,):
            return None
        if err not in (_errno.EINPROGRESS, _errno.EALREADY, _errno.EWOULDBLOCK):
            raise OSError(err, "connect")
        await _io_wait(sock.fileno(), 2)
        with _NonBlockingSocket(sock):
            err = sock.connect_ex(address)
        if err != 0:
            raise OSError(err, "connect")

    async def sock_accept(self, sock: Any) -> tuple[Any, Any]:
        if _molt_io_wait_new is None:
            # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio sock_accept.
            raise NotImplementedError("I/O polling unavailable")
        while True:
            with _NonBlockingSocket(sock):
                try:
                    return sock.accept()
                except (BlockingIOError, InterruptedError):
                    pass
                except OSError as exc:
                    if exc.errno not in (_errno.EAGAIN, _errno.EWOULDBLOCK):
                        raise
            await _io_wait(sock.fileno(), 1)

    def remove_writer(self, fd: Any) -> bool:
        fileno = _fd_from_fileobj(fd)
        entry = self._writers.pop(fileno, None)
        if entry is None:
            return False
        _callback, _args, task = entry
        try:
            task.cancel()
        except Exception:
            pass
        return True

    def run_until_complete(self, future: Any) -> Any:
        if self._closed:
            raise RuntimeError("Event loop is closed")
        if self._running:
            raise RuntimeError("Event loop is already running")
        global _RUNNING_LOOP
        prev = _RUNNING_LOOP
        _RUNNING_LOOP = self
        self._running = True
        self._stopping = False
        if self._ready:
            self._ensure_ready_runner()
        result: Any = None
        try:
            if isinstance(future, Future):
                fut = future
                if isinstance(fut, Task) and not getattr(fut, "_runner_spawned", True):
                    prev_token_id = _swap_current_token(fut._token)
                    try:
                        runner = fut._runner(fut.get_coro())
                        fut._runner_task = runner
                        try:
                            molt_task_register_token_owned(  # type: ignore[name-defined]
                                runner, fut._token.token_id()
                            )
                        except Exception:
                            pass
                        molt_block_on(runner)
                        _debug_exc_state("run_until_complete_after_block_on")
                        result = fut.result()
                        _debug_exc_state("run_until_complete_after_result")
                    finally:
                        _restore_token_id(prev_token_id)
                else:
                    result = molt_block_on(fut._wait())
                    _debug_exc_state("run_until_complete_after_wait")
            else:
                fut = Task(future, loop=self, _spawn_runner=False)
                prev_token_id = _swap_current_token(fut._token)
                try:
                    runner = fut._runner(fut.get_coro())
                    fut._runner_task = runner
                    try:
                        molt_task_register_token_owned(  # type: ignore[name-defined]
                            runner, fut._token.token_id()
                        )
                    except Exception:
                        pass
                    molt_block_on(runner)
                    _debug_exc_state("run_until_complete_after_block_on")
                    result = fut.result()
                    _debug_exc_state("run_until_complete_after_result")
                finally:
                    _restore_token_id(prev_token_id)
        finally:
            self._running = False
            self._stopping = False
            _RUNNING_LOOP = prev
        _debug_exc_state("run_until_complete_return")
        return result


class BaseEventLoop(_EventLoop):
    pass


class SelectorEventLoop(_EventLoop):
    def __init__(self, selector: Any | None = None) -> None:
        super().__init__(selector)


class _ProactorEventLoop(_EventLoop):
    pass


class AbstractEventLoopPolicy:
    """Base class for event loop policies."""

    def get_event_loop(self) -> EventLoop:
        raise NotImplementedError("get_event_loop not implemented")

    def set_event_loop(self, loop: EventLoop | None) -> None:
        raise NotImplementedError("set_event_loop not implemented")

    def new_event_loop(self) -> EventLoop:
        raise NotImplementedError("new_event_loop not implemented")


class DefaultEventLoopPolicy(AbstractEventLoopPolicy):
    def get_event_loop(self) -> EventLoop:
        global _EVENT_LOOP
        if _EVENT_LOOP is None:
            _EVENT_LOOP = _EventLoop()
        return _EVENT_LOOP

    def set_event_loop(self, loop: EventLoop | None) -> None:
        global _EVENT_LOOP
        _EVENT_LOOP = loop

    def new_event_loop(self) -> EventLoop:
        loop_cls = _EventLoop
        return loop_cls()


class _UnixDefaultEventLoopPolicy(DefaultEventLoopPolicy):
    pass


class _WindowsSelectorEventLoopPolicy(DefaultEventLoopPolicy):
    pass


class _WindowsProactorEventLoopPolicy(DefaultEventLoopPolicy):
    pass


def _default_event_loop_policy() -> AbstractEventLoopPolicy:
    if _IS_WINDOWS:
        return DefaultEventLoopPolicy()
    return _UnixDefaultEventLoopPolicy()


class AbstractChildWatcher:
    def __init__(self) -> None:
        self._loop: EventLoop | None = None
        self._callbacks: dict[int, tuple[Any, tuple[Any, ...]]] = {}

    def attach_loop(self, loop: EventLoop | None) -> None:
        self._loop = loop

    def add_child_handler(self, pid: int, callback: Any, *args: Any) -> None:
        self._callbacks[int(pid)] = (callback, args)

    def remove_child_handler(self, pid: int) -> bool:
        return self._callbacks.pop(int(pid), None) is not None

    def close(self) -> None:
        self._callbacks.clear()
        self._loop = None

    def is_active(self) -> bool:
        return self._loop is not None

    def _notify_child_exit(self, pid: int, returncode: int) -> None:
        entry = self._callbacks.pop(int(pid), None)
        if entry is None:
            return
        callback, args = entry
        try:
            callback(int(pid), int(returncode), *args)
        except Exception:
            pass


class SafeChildWatcher(AbstractChildWatcher):
    pass


class FastChildWatcher(AbstractChildWatcher):
    pass


class ThreadedChildWatcher(AbstractChildWatcher):
    pass


class PidfdChildWatcher(AbstractChildWatcher):
    pass


_CHILD_WATCHER: AbstractChildWatcher | None = None


def get_child_watcher() -> AbstractChildWatcher:
    if _IS_WINDOWS:
        raise NotImplementedError("child watchers not supported on Windows")
    global _CHILD_WATCHER
    if _CHILD_WATCHER is None:
        _CHILD_WATCHER = SafeChildWatcher()
    loop = _get_running_loop()
    if loop is not None:
        _CHILD_WATCHER.attach_loop(loop)
    return _CHILD_WATCHER


def set_child_watcher(watcher: AbstractChildWatcher | None) -> None:
    if _IS_WINDOWS:
        raise NotImplementedError("child watchers not supported on Windows")
    global _CHILD_WATCHER
    if watcher is None:
        _CHILD_WATCHER = None
        return None
    if not isinstance(watcher, AbstractChildWatcher):
        raise TypeError("watcher must be an AbstractChildWatcher")
    loop = _get_running_loop()
    if loop is not None:
        watcher.attach_loop(loop)
    _CHILD_WATCHER = watcher
    return None


EventLoop = _EventLoop
if _EXPOSE_WINDOWS_POLICIES:
    ProactorEventLoop = _ProactorEventLoop
    WindowsSelectorEventLoopPolicy = _WindowsSelectorEventLoopPolicy
    WindowsProactorEventLoopPolicy = _WindowsProactorEventLoopPolicy
if _EXPOSE_QUEUE_SHUTDOWN:
    QueueShutDown = _QueueShutDown


class Transport:
    pass


class Protocol:
    pass


class BaseProtocol(Protocol):
    pass


class BufferedProtocol(Protocol):
    pass


class DatagramProtocol(Protocol):
    pass


class StreamReaderProtocol(Protocol):
    pass


class SubprocessProtocol(Protocol):
    pass


class DatagramTransport(Transport):
    pass


class SubprocessTransport(Transport):
    pass


_EVENT_LOOP_POLICY: AbstractEventLoopPolicy = _default_event_loop_policy()
_EVENT_LOOP: EventLoop | None = None
_RUNNING_LOOP: EventLoop | None = None


def _get_running_loop() -> EventLoop | None:
    return _RUNNING_LOOP


def _set_running_loop(loop: EventLoop | None) -> None:
    global _RUNNING_LOOP
    _RUNNING_LOOP = loop


def get_running_loop() -> EventLoop:
    loop = _get_running_loop()
    if loop is None:
        raise RuntimeError("no running event loop")
    return loop


def get_event_loop_policy() -> AbstractEventLoopPolicy:
    return _EVENT_LOOP_POLICY


def set_event_loop_policy(policy: AbstractEventLoopPolicy | None) -> None:
    global _EVENT_LOOP_POLICY
    if policy is None:
        policy = _default_event_loop_policy()
    _EVENT_LOOP_POLICY = policy


def get_event_loop() -> EventLoop:
    return _EVENT_LOOP_POLICY.get_event_loop()


def set_event_loop(loop: EventLoop | None) -> None:
    _EVENT_LOOP_POLICY.set_event_loop(loop)


def new_event_loop() -> EventLoop:
    return _EVENT_LOOP_POLICY.new_event_loop()


def _cancel_all_tasks(loop: EventLoop) -> None:
    tasks = [task for task in all_tasks(loop) if not task.done()]
    if not tasks:
        return
    for task in tasks:
        task.cancel()
    try:
        loop.run_until_complete(gather(*tasks, return_exceptions=True))
    except BaseException:
        pass


class Runner:
    def __init__(self, *, debug: bool | None = None) -> None:
        self._loop: EventLoop | None = None
        self._debug = debug

    def __enter__(self) -> "Runner":
        if self._loop is None:
            self._loop = new_event_loop()
        if self._debug is not None:
            self._loop.set_debug(self._debug)
        set_event_loop(self._loop)
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    def get_loop(self) -> EventLoop:
        if self._loop is None:
            raise RuntimeError("Runner is not initialized")
        return self._loop

    def run(self, coro: Any) -> Any:
        if self._loop is None:
            self.__enter__()
        loop = self.get_loop()
        if loop.is_running():
            raise RuntimeError("Runner loop is already running")
        try:
            result = loop.run_until_complete(coro)
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
            _cancel_all_tasks(self._loop)
            shutdown = globals().get("molt_asyncgen_shutdown")
            if shutdown is not None:
                shutdown()
            self._loop.close()
        set_event_loop(None)


def run(awaitable: Any) -> Any:
    if _RUNNING_LOOP is not None:
        raise RuntimeError("asyncio.run() cannot be called from a running event loop")
    runner = Runner()
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
        await molt_async_sleep(0.0)
        return result
    loop = get_running_loop()
    fut = loop.create_future()

    def _wake() -> None:
        if not fut.done():
            fut.set_result(result)

    handle = loop.call_later(delay, _wake)
    try:
        return await fut
    finally:
        if not fut.done():
            handle.cancel()


async def open_connection(
    host: str,
    port: int,
    *,
    ssl: Any | None = None,
    local_addr: Any | None = None,
) -> tuple["StreamReader", "StreamWriter"]:
    if ssl is not None:
        # TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:missing): implement asyncio SSL transport support.
        raise NotImplementedError("ssl not supported")
    if _molt_io_wait_new is None:
        # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio open_connection.
        raise NotImplementedError("I/O polling unavailable")
    sock = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM)
    if local_addr is not None:
        sock.bind(local_addr)
    with _NonBlockingSocket(sock):
        err = sock.connect_ex((host, port))
    if err not in (0, _errno.EINPROGRESS, _errno.EALREADY, _errno.EWOULDBLOCK):
        raise OSError(err, "connect")
    if err != 0:
        await _io_wait(sock.fileno(), 2)
        with _NonBlockingSocket(sock):
            err = sock.connect_ex((host, port))
        if err != 0:
            raise OSError(err, "connect")
    reader = StreamReader(sock)
    writer = StreamWriter(sock)
    return reader, writer


async def open_unix_connection(
    path: str,
    *,
    ssl: Any | None = None,
    local_addr: Any | None = None,
) -> tuple["StreamReader", "StreamWriter"]:
    if _os.name == "nt" or not hasattr(_socket, "AF_UNIX"):
        raise NotImplementedError("unix sockets not supported")
    if ssl is not None:
        # TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:missing): implement asyncio SSL transport support for unix sockets.
        raise NotImplementedError("ssl not supported")
    if _molt_io_wait_new is None:
        # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio open_unix_connection.
        raise NotImplementedError("I/O polling unavailable")
    sock = _socket.socket(_socket.AF_UNIX, _socket.SOCK_STREAM)
    if local_addr is not None:
        sock.bind(local_addr)
    with _NonBlockingSocket(sock):
        err = sock.connect_ex(path)
    if err not in (0, _errno.EINPROGRESS, _errno.EALREADY, _errno.EWOULDBLOCK):
        raise OSError(err, "connect")
    if err != 0:
        await _io_wait(sock.fileno(), 2)
        with _NonBlockingSocket(sock):
            err = sock.connect_ex(path)
        if err != 0:
            raise OSError(err, "connect")
    reader = StreamReader(sock)
    writer = StreamWriter(sock)
    return reader, writer


async def to_thread(func: Any, /, *args: Any, **kwargs: Any) -> Any:
    loop = get_running_loop()
    ctx = _contextvars.copy_context()

    def _runner() -> Any:
        return ctx.run(func, *args, **kwargs)

    return await loop.run_in_executor(None, _runner)


async def start_server(
    client_connected_cb: Any,
    host: str | None = None,
    port: int | None = None,
    *,
    backlog: int = 100,
    reuse_port: bool = False,
) -> Server:
    if _molt_io_wait_new is None:
        # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio start_server.
        raise NotImplementedError("I/O polling unavailable")
    bind_host = host if host is not None else "0.0.0.0"
    bind_port = 0 if port is None else port
    sock = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM)
    sock.setsockopt(_socket.SOL_SOCKET, _socket.SO_REUSEADDR, 1)
    if reuse_port and hasattr(_socket, "SO_REUSEPORT"):
        sock.setsockopt(_socket.SOL_SOCKET, int(getattr(_socket, "SO_REUSEPORT")), 1)
    sock.setblocking(False)
    sock.bind((bind_host, bind_port))
    sock.listen(backlog)
    return Server(sock, client_connected_cb)


async def start_unix_server(
    client_connected_cb: Any,
    path: str,
    *,
    backlog: int = 100,
) -> Server:
    if _os.name == "nt" or not hasattr(_socket, "AF_UNIX"):
        raise NotImplementedError("unix sockets not supported")
    if _molt_io_wait_new is None:
        # TODO(runtime, owner:runtime, milestone:RT2, priority:P0, status:missing): require io_wait intrinsic for asyncio start_unix_server.
        raise NotImplementedError("I/O polling unavailable")
    sock = _socket.socket(_socket.AF_UNIX, _socket.SOCK_STREAM)
    sock.setblocking(False)
    sock.bind(path)
    sock.listen(backlog)
    return Server(sock, client_connected_cb)


async def create_subprocess_exec(
    *program: Any,
    stdin: Any | None = None,
    stdout: Any | None = None,
    stderr: Any | None = None,
    cwd: Any | None = None,
    env: Any | None = None,
) -> Process:
    if not program:
        raise ValueError("program must not be empty")
    stdin_mode = _normalize_proc_stdio(stdin, allow_stdout=False)
    stdout_mode = _normalize_proc_stdio(stdout, allow_stdout=False)
    stderr_mode = _normalize_proc_stdio(stderr, allow_stdout=True)
    if stderr_mode == subprocess.STDOUT:
        # TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:missing): wire stderr=STDOUT redirection in process spawn.
        raise NotImplementedError("stderr=STDOUT not supported")
    spawn = _require_asyncio_intrinsic(_molt_process_spawn, "process_spawn")
    handle = spawn(list(program), env, cwd, stdin_mode, stdout_mode, stderr_mode)
    if handle is None:
        raise PermissionError("Missing process capability")
    stdin_stream = (
        ProcessStreamWriter(
            _require_asyncio_intrinsic(_molt_process_stdin, "process_stdin")(handle)
        )
        if stdin_mode == _PROC_STDIO_PIPE
        else None
    )
    stdout_stream = (
        ProcessStreamReader(
            _require_asyncio_intrinsic(_molt_process_stdout, "process_stdout")(handle)
        )
        if stdout_mode == _PROC_STDIO_PIPE
        else None
    )
    stderr_stream = (
        ProcessStreamReader(
            _require_asyncio_intrinsic(_molt_process_stderr, "process_stderr")(handle)
        )
        if stderr_mode == _PROC_STDIO_PIPE
        else None
    )
    return Process(handle, stdin_stream, stdout_stream, stderr_stream)


async def create_subprocess_shell(
    cmd: str,
    stdin: Any | None = None,
    stdout: Any | None = None,
    stderr: Any | None = None,
    cwd: Any | None = None,
    env: Any | None = None,
) -> Process:
    if _os.name == "nt":
        program = ["cmd.exe", "/c", cmd]
    else:
        program = ["/bin/sh", "-c", cmd]
    return await create_subprocess_exec(
        *program,
        stdin=stdin,
        stdout=stdout,
        stderr=stderr,
        cwd=cwd,
        env=env,
    )


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
            try:
                fut._rebind_token(shield_token)
                molt_task_register_token_owned(  # type: ignore[name-defined]
                    fut._coro, shield_token.token_id()
                )
                setattr(fut, "__molt_shield_token__", shield_token)

                def _clear_shield_token(done: Future) -> None:
                    try:
                        delattr(done, "__molt_shield_token__")
                    except Exception:
                        pass

                fut.add_done_callback(_clear_shield_token)
            except Exception:
                pass
    try:
        return await fut
    except BaseException as exc:
        if _is_cancelled_exc(exc):
            raise
        raise


def create_eager_task_factory() -> Callable[[EventLoop, Any], Task]:
    def _factory(
        loop: EventLoop,
        coro: Any,
        *,
        name: str | None = None,
        context: Any | None = None,
    ) -> Task:
        return Task(coro, loop=loop, name=name, context=context)

    return _factory


def eager_task_factory(
    loop: EventLoop,
    coro: Any,
    *,
    name: str | None = None,
    context: Any | None = None,
) -> Task:
    return Task(coro, loop=loop, name=name, context=context)


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

    def _transfer() -> None:
        try:
            if hasattr(fut, "cancelled") and fut.cancelled():
                proxy.cancel()
                return
            if hasattr(fut, "exception"):
                exc = fut.exception()
                if exc is not None:
                    proxy.set_exception(exc)
                    return
            if hasattr(fut, "result"):
                proxy.set_result(fut.result())
                return
        except BaseException as exc:
            proxy.set_exception(exc)
            return
        proxy.set_result(None)

    try:
        if hasattr(fut, "add_done_callback"):
            fut.add_done_callback(lambda _fut: _transfer())
        else:
            loop.call_soon_threadsafe(_transfer)
    except BaseException as exc:
        proxy.set_exception(exc)
    return proxy


def current_task(loop: EventLoop | None = None) -> Task | None:
    if loop is None:
        loop = get_running_loop()
    task = _TASKS.get(_current_token_id())
    if (
        loop is not None
        and task is not None
        and getattr(task, "_loop", None) is not loop
    ):
        return None
    return task


def all_tasks(loop: EventLoop | None = None) -> set[Task]:
    tasks = {
        task for task in _TASKS.values() if isinstance(task, Task) and not task.done()
    }
    if loop is None:
        loop = get_running_loop()
    return {task for task in tasks if getattr(task, "_loop", None) is loop}


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
    tasks = [ensure_future(aw) for aw in aws]
    if not tasks:
        raise ValueError("asyncio.wait() requires at least one awaitable")
    if return_when not in (ALL_COMPLETED, FIRST_COMPLETED, FIRST_EXCEPTION):
        raise ValueError("Invalid return_when value")
    done: set[Future] = set()
    pending: set[Future] = set(tasks)

    def update_done() -> bool:
        triggered = False
        for task in list(pending):
            if _future_done(task):
                pending.remove(task)
                done.add(task)
                if return_when is FIRST_COMPLETED:
                    triggered = True
                elif return_when is FIRST_EXCEPTION:
                    if _future_cancelled(task) or _future_exception(task) is not None:
                        triggered = True
        return triggered

    if timeout is None:
        while pending:
            if update_done():
                break
            await sleep(0.0)
        return done, pending

    timeout_val = float(timeout)
    if timeout_val <= 0.0:
        if pending:
            loop = get_running_loop()
            waiter = loop.create_future()

            def _wake() -> None:
                if not waiter.done():
                    waiter.set_result(None)

            def _on_done(_task: Future) -> None:
                _wake()

            for task in list(pending):
                try:
                    task.add_done_callback(_on_done)  # type: ignore[union-attr]
                except Exception:
                    pass
            loop.call_later(_WAIT_TIMEOUT_EPS, _wake)
            await waiter
        update_done()
        return done, pending

    timer = ensure_future(sleep(timeout_val))
    try:
        while pending:
            if timer.done():
                break
            if update_done():
                break
            await sleep(0.0)
    finally:
        timer.cancel()
    return done, pending


async def wait_for(awaitable: Any, timeout: float | None) -> Any:
    async def _cancel_and_wait(fut: Future, *, force_timeout: bool = False) -> Any:
        fut.cancel()
        while not fut.done():
            await sleep(0.0)
        if fut.cancelled():
            raise TimeoutError
        if force_timeout:
            raise TimeoutError
        return fut.result()

    if timeout is None:
        fut = ensure_future(awaitable)
        return await fut
    fut = ensure_future(awaitable)
    if fut.done():
        return fut.result()
    timeout_val = float(timeout)
    if timeout_val <= 0.0:
        if _DEBUG_WAIT_FOR:
            _debug_write("wait_for_debug: immediate timeout")
        await sleep(0.0)
        return await _cancel_and_wait(fut, force_timeout=True)
    timer = ensure_future(sleep(timeout_val))
    debug_loops = 0
    try:
        while True:
            if fut.done():
                timer.cancel()
                if _DEBUG_WAIT_FOR:
                    _debug_write(
                        "wait_for_debug: target done cancelled={}".format(
                            fut.cancelled()
                        )
                    )
                return fut.result()
            if timer.done():
                if _DEBUG_WAIT_FOR:
                    _debug_write("wait_for_debug: timer done, cancel target")
                timer.cancel()
                return await _cancel_and_wait(fut)
            if _DEBUG_WAIT_FOR:
                debug_loops += 1
                if debug_loops % 1000 == 0:
                    _debug_write(
                        "wait_for_debug loops={loops} fut_done={fut_done} timer_done={timer_done}".format(
                            loops=debug_loops,
                            fut_done=fut.done(),
                            timer_done=timer.done(),
                        )
                    )
            await sleep(0.0)
    except BaseException as exc:
        if _is_cancelled_exc(exc):
            fut.cancel()
            timer.cancel()
        raise


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
    index = {task: idx for idx, task in enumerate(tasks)}
    results: list[Any] = [None] * len(tasks)
    pending: set[Future] = set(tasks)
    try:
        while pending:
            done, pending = await wait(pending, return_when=FIRST_COMPLETED)
            for task in done:
                idx = index[task]
                if _future_cancelled(task):
                    exc = CancelledError()
                else:
                    exc = _future_exception(task)
                if exc is not None:
                    if return_exceptions:
                        results[idx] = exc
                        continue
                    for remaining in pending:
                        remaining.cancel()
                    raise exc
                results[idx] = task.result()
    except BaseException as exc:
        if _is_cancelled_exc(exc):
            for task in tasks:
                task.cancel()
        raise
    return results


async def _wait_one(queue: "Queue", timeout: float | None) -> Any:
    if timeout is None:
        task = await queue.get()
        return await task
    return await wait_for(queue.get(), timeout)


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

    def __iter__(self) -> "_AsCompletedIterator":
        return self

    def __next__(self) -> Any:
        if self._remaining <= 0:
            raise StopIteration
        self._remaining -= 1
        return _wait_one(self._queue, self._timeout)


def as_completed(aws: Iterable[Any], timeout: float | None = None) -> Iterator[Any]:
    tasks = [ensure_future(aw) for aw in aws]
    queue: Queue = Queue()

    def _enqueue(task: Future, _queue: "Queue" = queue) -> None:
        try:
            _queue.put_nowait(task)
        except Exception:
            pass

    for task in tasks:
        task.add_done_callback(_enqueue)

    return _AsCompletedIterator(tasks, queue, timeout)


class Queue:
    def __init__(self, maxsize: int = 0) -> None:
        if maxsize < 0:
            raise ValueError("maxsize must be >= 0")
        self._maxsize = maxsize
        self._getters: _deque[Future] = _deque()
        self._putters: _deque[Future] = _deque()
        self._unfinished_tasks = 0
        self._finished = Event()
        self._finished.set()
        self._shutdown = False
        self._init()

    def _init(self) -> None:
        self._queue: Any = _deque()

    def qsize(self) -> int:
        return len(self._queue)

    def empty(self) -> bool:
        return not self._queue

    def full(self) -> bool:
        return self._maxsize > 0 and len(self._queue) >= self._maxsize

    async def put(self, item: Any) -> None:
        if self._shutdown:
            raise _QueueShutDown
        while self.full():
            fut = Future()
            self._putters.append(fut)
            try:
                await fut
            except BaseException as exc:
                if _is_cancelled_exc(exc):
                    try:
                        self._putters.remove(fut)
                    except ValueError:
                        pass
                raise
            if self._shutdown:
                raise _QueueShutDown
        self._put_nowait(item)

    def put_nowait(self, item: Any) -> None:
        if self._shutdown:
            raise _QueueShutDown
        if self.full():
            raise QueueFull
        self._put_nowait(item)

    def _put_nowait(self, item: Any) -> None:
        self._unfinished_tasks += 1
        if self._finished.is_set():
            self._finished.clear()
        if self._getters:
            getter = self._getters.popleft()
            if not getter.done():
                getter.set_result(item)
        else:
            self._put(item)

    def _put(self, item: Any) -> None:
        self._queue.append(item)

    async def get(self) -> Any:
        if self._queue:
            return self._get_nowait()
        if self._shutdown:
            raise _QueueShutDown
        fut = Future()
        self._getters.append(fut)
        try:
            return await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                try:
                    self._getters.remove(fut)
                except ValueError:
                    pass
            raise

    def get_nowait(self) -> Any:
        if self._queue:
            return self._get_nowait()
        if self._shutdown:
            raise _QueueShutDown
        raise QueueEmpty

    def _get_nowait(self) -> Any:
        item = self._get()
        if self._putters:
            putter = self._putters.popleft()
            if not putter.done():
                putter.set_result(True)
        return item

    def _get(self) -> Any:
        return self._queue.popleft()

    def task_done(self) -> None:
        if self._unfinished_tasks <= 0:
            raise ValueError("task_done() called too many times")
        self._unfinished_tasks -= 1
        if self._unfinished_tasks == 0:
            self._finished.set()

    async def join(self) -> None:
        await self._finished.wait()

    if _EXPOSE_QUEUE_SHUTDOWN:

        def shutdown(self) -> None:
            self._shutdown = True
            while self._getters:
                getter = self._getters.popleft()
                if not getter.done():
                    getter.set_exception(_QueueShutDown())
            while self._putters:
                putter = self._putters.popleft()
                if not putter.done():
                    putter.set_exception(_QueueShutDown())


class PriorityQueue(Queue):
    def _init(self) -> None:
        self._queue = []

    def _put(self, item: Any) -> None:
        _heapq.heappush(self._queue, item)

    def _get(self) -> Any:
        return _heapq.heappop(self._queue)


class LifoQueue(Queue):
    def _init(self) -> None:
        self._queue = []

    def _put(self, item: Any) -> None:
        self._queue.append(item)

    def _get(self) -> Any:
        return self._queue.pop()


def _module(name: str, attrs: dict[str, Any]) -> _types.ModuleType:
    if callable(_molt_module_new):
        mod = _molt_module_new(name)  # type: ignore[misc]
    else:
        mod = _types.ModuleType(name)
    mod_dict = getattr(mod, "__dict__", None)
    if isinstance(mod_dict, dict):
        mod_dict.update(attrs)
    else:
        for key, val in attrs.items():
            try:
                setattr(mod, key, val)
            except Exception:
                pass
    try:
        mod.__name__ = name
        mod.__package__ = name.rpartition(".")[0]
    except Exception:
        pass
    try:
        _sys.modules[name] = mod
    except Exception:
        pass
    return mod


def _format_callback_source(
    callback: Any, args: tuple[Any, ...], *, debug: bool = False
) -> str:
    name = getattr(callback, "__qualname__", None) or getattr(
        callback, "__name__", None
    )
    if name is None:
        name = repr(callback)
    if args:
        args_repr = ", ".join(repr(arg) for arg in args)
        return f"{name}({args_repr})"
    return f"{name}()"


def _extract_stack(limit: int | None = None) -> list[Any]:
    return _traceback.extract_stack(limit=limit)


class FlowControlMixin:
    def __init__(self) -> None:
        self._paused = False

    def pause_writing(self) -> None:
        self._paused = True

    def resume_writing(self) -> None:
        self._paused = False


_coroutine = getattr(_types, "coroutine", None)
if _coroutine is None:

    def _coroutine(func: Any) -> Any:
        return func


events = _module(
    "asyncio.events",
    {
        "AbstractEventLoop": AbstractEventLoop,
        "AbstractEventLoopPolicy": AbstractEventLoopPolicy,
        "AbstractServer": AbstractServer,
        "BaseEventLoop": BaseEventLoop,
        "DefaultEventLoopPolicy": DefaultEventLoopPolicy,
        "Handle": Handle,
        "TimerHandle": TimerHandle,
        "_get_running_loop": _get_running_loop,
        "_set_running_loop": _set_running_loop,
        "get_event_loop": get_event_loop,
        "get_event_loop_policy": get_event_loop_policy,
        "get_running_loop": get_running_loop,
        "new_event_loop": new_event_loop,
        "set_event_loop": set_event_loop,
        "set_event_loop_policy": set_event_loop_policy,
    },
)

base_events = _module(
    "asyncio.base_events",
    {
        "BaseEventLoop": BaseEventLoop,
        "AbstractServer": AbstractServer,
        "SelectorEventLoop": SelectorEventLoop,
        "Handle": Handle,
        "TimerHandle": TimerHandle,
        "Server": Server,
    },
)

constants = _module(
    "asyncio.constants",
    {
        "LOG_THRESHOLD_FOR_CONNLOST_WRITES": 5,
        "ACCEPT_RETRY_DELAY": 1,
        "SLOW_CALLBACK_DURATION": 0.1,
        "DEFAULT_LIMIT": 2**16,
    },
)

coroutines = _module(
    "asyncio.coroutines",
    {
        "iscoroutine": iscoroutine,
        "iscoroutinefunction": iscoroutinefunction,
        "coroutine": _coroutine,
    },
)

exceptions = _module(
    "asyncio.exceptions",
    {
        "CancelledError": CancelledError,
        "InvalidStateError": InvalidStateError,
        "TimeoutError": TimeoutError,
        "SendfileNotAvailableError": SendfileNotAvailableError,
        "IncompleteReadError": IncompleteReadError,
        "LimitOverrunError": LimitOverrunError,
        "QueueEmpty": QueueEmpty,
        "QueueFull": QueueFull,
        "BrokenBarrierError": BrokenBarrierError,
    },
)
if _EXPOSE_QUEUE_SHUTDOWN:
    try:
        setattr(exceptions, "QueueShutDown", _QueueShutDown)
    except Exception:
        pass

format_helpers = _module(
    "asyncio.format_helpers",
    {
        "_format_callback_source": _format_callback_source,
        "extract_stack": _extract_stack,
    },
)


def _queues_attrs() -> dict[str, Any]:
    attrs = {
        "Queue": Queue,
        "PriorityQueue": PriorityQueue,
        "LifoQueue": LifoQueue,
        "QueueEmpty": QueueEmpty,
        "QueueFull": QueueFull,
    }
    if _EXPOSE_QUEUE_SHUTDOWN:
        attrs["QueueShutDown"] = _QueueShutDown
    return attrs


def _make_log_module() -> _types.ModuleType:
    class _NoopLogger:
        def log(self, *args: Any, **kwargs: Any) -> None:
            return None

        def debug(self, *args: Any, **kwargs: Any) -> None:
            return None

        def info(self, *args: Any, **kwargs: Any) -> None:
            return None

        def warning(self, *args: Any, **kwargs: Any) -> None:
            return None

        def error(self, *args: Any, **kwargs: Any) -> None:
            return None

        def exception(self, *args: Any, **kwargs: Any) -> None:
            return None

        def critical(self, *args: Any, **kwargs: Any) -> None:
            return None

    try:
        logger = _logging.getLogger("asyncio")
    except Exception:
        logger = _NoopLogger()
    return _module(
        "asyncio.log",
        {
            "logger": logger,
        },
    )


def _make_mixins_module() -> _types.ModuleType:
    return _module(
        "asyncio.mixins",
        {
            "FlowControlMixin": FlowControlMixin,
        },
    )


def _make_locks_module() -> _types.ModuleType:
    return _module(
        "asyncio.locks",
        {
            "Lock": Lock,
            "Event": Event,
            "Condition": Condition,
            "Semaphore": Semaphore,
            "BoundedSemaphore": BoundedSemaphore,
            "Barrier": Barrier,
            "BrokenBarrierError": BrokenBarrierError,
        },
    )


def _make_queues_module() -> _types.ModuleType:
    return _module("asyncio.queues", _queues_attrs())


try:
    log = _make_log_module()
except Exception:
    log = None
try:
    mixins = _make_mixins_module()
except Exception:
    mixins = None
try:
    locks = _make_locks_module()
except Exception:
    locks = None
try:
    queues = _make_queues_module()
except Exception:
    queues = None

if log is None:
    try:
        del log
    except Exception:
        pass
if mixins is None:
    try:
        del mixins
    except Exception:
        pass
if locks is None:
    try:
        del locks
    except Exception:
        pass
if queues is None:
    try:
        del queues
    except Exception:
        pass


def __getattr__(name: str) -> Any:
    if name == "log":
        mod = _make_log_module()
    elif name == "mixins":
        mod = _make_mixins_module()
    elif name == "locks":
        mod = _make_locks_module()
    elif name == "queues":
        mod = _make_queues_module()
    else:
        raise AttributeError(f"module 'asyncio' has no attribute '{name}'")
    globals()[name] = mod
    return mod


protocols = _module(
    "asyncio.protocols",
    {
        "BaseProtocol": BaseProtocol,
        "Protocol": Protocol,
        "BufferedProtocol": BufferedProtocol,
        "DatagramProtocol": DatagramProtocol,
        "StreamReaderProtocol": StreamReaderProtocol,
        "SubprocessProtocol": SubprocessProtocol,
    },
)

transports = _module(
    "asyncio.transports",
    {
        "Transport": Transport,
        "DatagramTransport": DatagramTransport,
        "SubprocessTransport": SubprocessTransport,
    },
)

runners = _module(
    "asyncio.runners",
    {
        "Runner": Runner,
        "run": run,
    },
)

taskgroups = _module(
    "asyncio.taskgroups",
    {
        "TaskGroup": TaskGroup,
    },
)

threads = _module(
    "asyncio.threads",
    {
        "to_thread": to_thread,
    },
)

timeouts = _module(
    "asyncio.timeouts",
    {
        "timeout": timeout,
        "timeout_at": timeout_at,
        "TimeoutError": TimeoutError,
    },
)

base_futures = _module(
    "asyncio.base_futures",
    {
        "Future": Future,
        "CancelledError": CancelledError,
        "InvalidStateError": InvalidStateError,
    },
)

base_tasks = _module(
    "asyncio.base_tasks",
    {
        "Task": Task,
        "current_task": current_task,
        "all_tasks": all_tasks,
    },
)

base_subprocess = _module(
    "asyncio.base_subprocess",
    {
        "Process": Process,
    },
)

selector_events = _module(
    "asyncio.selector_events",
    {
        "BaseSelectorEventLoop": SelectorEventLoop,
        "SelectorEventLoop": SelectorEventLoop,
        "DefaultEventLoopPolicy": DefaultEventLoopPolicy,
    },
)
if _EXPOSE_EVENT_LOOP:
    try:
        setattr(selector_events, "EventLoop", SelectorEventLoop)
    except Exception:
        pass

sslproto = _module("asyncio.sslproto", {})

subprocess = _module(
    "asyncio.subprocess",
    {
        "PIPE": _PROC_STDIO_PIPE,
        "STDOUT": _SubprocessConstants.STDOUT,
        "DEVNULL": _PROC_STDIO_DEVNULL,
        "Process": Process,
        "SubprocessProtocol": SubprocessProtocol,
        "SubprocessTransport": SubprocessTransport,
        "create_subprocess_exec": create_subprocess_exec,
        "create_subprocess_shell": create_subprocess_shell,
    },
)

_futures_attrs: dict[str, Any] = {
    "Future": Future,
    "CancelledError": CancelledError,
    "InvalidStateError": InvalidStateError,
    "wrap_future": wrap_future,
}
if _EXPOSE_GRAPH:
    _futures_attrs.update(
        {
            "future_add_to_awaited_by": future_add_to_awaited_by,
            "future_discard_from_awaited_by": future_discard_from_awaited_by,
        }
    )
futures = _module("asyncio.futures", _futures_attrs)

tasks = _module(
    "asyncio.tasks",
    {
        "Task": Task,
        "TaskGroup": TaskGroup,
        "all_tasks": all_tasks,
        "as_completed": as_completed,
        "create_eager_task_factory": create_eager_task_factory,
        "create_task": create_task,
        "current_task": current_task,
        "eager_task_factory": eager_task_factory,
        "ensure_future": ensure_future,
        "gather": gather,
        "run_coroutine_threadsafe": run_coroutine_threadsafe,
        "shield": shield,
        "sleep": sleep,
        "wait": wait,
        "wait_for": wait_for,
    },
)

streams = _module(
    "asyncio.streams",
    {
        "StreamReader": StreamReader,
        "StreamWriter": StreamWriter,
        "open_connection": open_connection,
        "open_unix_connection": open_unix_connection,
        "start_server": start_server,
        "start_unix_server": start_unix_server,
    },
)

trsock = _module("asyncio.trsock", {})

if not _IS_WINDOWS:
    unix_events = _module(
        "asyncio.unix_events",
        {
            "SelectorEventLoop": SelectorEventLoop,
        },
    )
    if _EXPOSE_EVENT_LOOP:
        try:
            setattr(unix_events, "EventLoop", SelectorEventLoop)
        except Exception:
            pass

if _IS_WINDOWS:
    windows_events = _module(
        "asyncio.windows_events",
        {
            "SelectorEventLoop": SelectorEventLoop,
            "ProactorEventLoop": _ProactorEventLoop,
            "DefaultEventLoopPolicy": _WindowsProactorEventLoopPolicy,
            "WindowsSelectorEventLoopPolicy": _WindowsSelectorEventLoopPolicy,
            "WindowsProactorEventLoopPolicy": _WindowsProactorEventLoopPolicy,
        },
    )
    if _EXPOSE_EVENT_LOOP:
        try:
            setattr(windows_events, "EventLoop", _ProactorEventLoop)
        except Exception:
            pass
    windows_utils = _module("asyncio.windows_utils", {})

staggered = _module("asyncio.staggered", {})

if _EXPOSE_GRAPH:
    graph = _module(
        "asyncio.graph",
        {
            "capture_call_graph": capture_call_graph,
            "format_call_graph": format_call_graph,
            "print_call_graph": print_call_graph,
            "FrameCallGraphEntry": FrameCallGraphEntry,
            "FutureCallGraph": FutureCallGraph,
        },
    )

if not _EXPOSE_EVENT_LOOP:
    try:
        del globals()["EventLoop"]
    except Exception:
        pass

if not _EXPOSE_GRAPH:
    for _name in (
        "capture_call_graph",
        "format_call_graph",
        "print_call_graph",
        "FrameCallGraphEntry",
        "FutureCallGraph",
        "future_add_to_awaited_by",
        "future_discard_from_awaited_by",
    ):
        try:
            del globals()[_name]
        except Exception:
            pass

_builtin_targets = [
    _get_running_loop,
    _set_running_loop,
    get_running_loop,
    get_event_loop,
]
if _EXPOSE_GRAPH:
    _builtin_targets.extend([future_add_to_awaited_by, future_discard_from_awaited_by])
for _fn in _builtin_targets:
    _mark_builtin(_fn)
