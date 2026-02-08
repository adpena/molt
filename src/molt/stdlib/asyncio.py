"""Capability-gated asyncio shim for Molt."""

from __future__ import annotations
from typing import TYPE_CHECKING, Any, Callable, Iterable, Iterator
from dataclasses import dataclass
import builtins as _builtins
from collections import deque as _deque
import heapq as _heapq
import logging as _logging
import os as _os
import sys as _sys
import time as _time
import traceback as _traceback
import errno as _errno
import socket as _socket
import types as _types
import threading as _threading

import contextvars as _contextvars

from molt.concurrency import CancellationToken, spawn

from _intrinsics import require_intrinsic as _intrinsic_require


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

    def molt_async_sleep(_delay: float = 0.0, _result: Any | None = None) -> Any: ...

    def molt_block_on(awaitable: Any) -> Any: ...

    def molt_asyncgen_shutdown() -> None: ...

    def molt_cancel_token_set_current(_token_id: int) -> int: ...

    def molt_promise_new() -> Any: ...

    def molt_promise_set_exception(_promise: Any, _exc: Any) -> None: ...

    def molt_promise_set_result(_promise: Any, _value: Any) -> None: ...

    def molt_cancel_token_get_current() -> int: ...

    def molt_task_register_token_owned(_task: Any, _token_id: int) -> None: ...

    def molt_future_cancel(_future: Any) -> None: ...

    def molt_asyncio_wait_for_new(_future: Any, _timeout: Any | None = None) -> Any: ...

    def molt_asyncio_wait_new(
        _tasks: Any, _timeout: Any | None = None, _return_when: int = 0
    ) -> Any: ...

    def molt_asyncio_gather_new(
        _tasks: Any, _return_exceptions: bool = False
    ) -> Any: ...

    def molt_asyncio_cancel_pending(_tasks: Any) -> int: ...

    def molt_asyncio_ready_batch_run(_handles: Any) -> int: ...

    def molt_asyncio_ready_queue_drain(_ready_lock: Any, _ready: Any) -> int: ...

    def molt_asyncio_waiters_notify(
        _waiters: Any, _count: int = 1, _result: Any = True
    ) -> int: ...

    def molt_asyncio_waiters_notify_exception(
        _waiters: Any, _count: int = 1, _exc: Any = None
    ) -> int: ...

    def molt_asyncio_waiters_remove(_waiters: Any, _waiter: Any) -> bool: ...

    def molt_asyncio_barrier_release(_waiters: Any) -> int: ...

    def molt_asyncio_condition_wait_for_step(
        _condition: Any, _predicate: Any
    ) -> tuple[bool, Any]: ...

    def molt_asyncio_future_transfer(_source: Any, _target: Any) -> bool: ...

    def molt_asyncio_event_waiters_cleanup(_waiters: Any) -> int: ...

    def molt_asyncio_task_registry_set(_token_id: int, _task: Any | None) -> None: ...

    def molt_asyncio_task_registry_get(_token_id: int) -> Any | None: ...

    def molt_asyncio_task_registry_contains(_token_id: int) -> bool: ...

    def molt_asyncio_task_registry_current() -> Any | None: ...

    def molt_asyncio_task_registry_current_for_loop(
        _loop: Any | None = None,
    ) -> Any | None: ...

    def molt_asyncio_task_registry_pop(_token_id: int) -> Any | None: ...

    def molt_asyncio_task_registry_move(
        _old_token_id: int, _new_token_id: int
    ) -> bool: ...

    def molt_asyncio_task_registry_values() -> Any: ...

    def molt_asyncio_task_registry_live(_loop: Any | None = None) -> Any: ...

    def molt_asyncio_task_registry_live_set(_loop: Any | None = None) -> Any: ...

    def molt_asyncio_event_waiters_register(_token_id: int, _waiter: Any) -> None: ...

    def molt_asyncio_event_waiters_unregister(_token_id: int, _waiter: Any) -> bool: ...

    def molt_asyncio_event_waiters_cleanup_token(_token_id: int) -> int: ...

    def molt_asyncio_child_watcher_add(
        _callbacks: Any, _pid: int, _callback: Any, _args: tuple[Any, ...]
    ) -> None: ...

    def molt_asyncio_child_watcher_remove(_callbacks: Any, _pid: int) -> bool: ...

    def molt_asyncio_child_watcher_clear(_callbacks: Any) -> None: ...

    def molt_asyncio_child_watcher_pop(
        _callbacks: Any, _pid: int
    ) -> tuple[Any, tuple[Any, ...]] | None: ...

    def molt_asyncio_require_ssl_transport_support() -> None: ...

    def molt_asyncio_ssl_transport_orchestrate(
        _operation: str,
        _ssl: Any,
        _server_hostname: str | None = None,
        _server_side: bool = False,
    ) -> bool: ...

    def molt_asyncio_tls_client_connect_new(
        _host: str, _port: int, _server_hostname: str | None = None
    ) -> Any: ...

    def molt_asyncio_tls_client_from_fd_new(
        _fd: int, _server_hostname: str | None = None
    ) -> Any: ...

    def molt_asyncio_tls_server_payload(_ssl: Any) -> tuple[str, str]: ...

    def molt_asyncio_tls_server_from_fd_new(
        _fd: int, _certfile: str, _keyfile: str
    ) -> Any: ...

    def molt_asyncio_require_unix_socket_support() -> None: ...

    def molt_asyncio_require_child_watcher_support() -> None: ...

    def molt_asyncio_running_loop_get() -> Any: ...

    def molt_asyncio_running_loop_set(_loop: Any) -> None: ...

    def molt_asyncio_event_loop_get() -> Any: ...

    def molt_asyncio_event_loop_set(_loop: Any) -> None: ...

    def molt_asyncio_event_loop_policy_get() -> Any: ...

    def molt_asyncio_event_loop_policy_set(_policy: Any) -> None: ...

    def molt_asyncio_taskgroup_on_task_done(
        _tasks: Any, _errors: Any, _task: Any
    ) -> bool: ...

    def molt_asyncio_taskgroup_request_cancel(
        _loop: Any | None, _cancel_callback: Any, _cancel_handle: Any | None = None
    ) -> Any | None: ...

    def molt_asyncio_tasks_add_done_callback(_tasks: Any, _callback: Any) -> int: ...

    def molt_asyncio_task_cancel_apply(
        _future: Any, _msg: Any | None = None
    ) -> bool: ...

    def molt_asyncio_task_uncancel_apply(_future: Any) -> None: ...

    def molt_asyncio_future_invoke_callbacks(_future: Any, _callbacks: Any) -> int: ...

    def molt_asyncio_event_set_waiters(_waiters: Any, _result: Any = True) -> int: ...

    def molt_asyncio_loop_enqueue_handle(
        _loop: Any, _ready_lock: Any, _ready: Any, _handle: Any
    ) -> int: ...

    def molt_asyncio_timer_handle_new(
        _handle: Any,
        _delay: Any,
        _loop: Any,
        _scheduled: Any,
        _ready_lock: Any,
        _ready: Any,
    ) -> Any: ...

    def molt_asyncio_timer_schedule(
        _handle: Any,
        _delay: Any,
        _loop: Any,
        _scheduled: Any,
        _ready_lock: Any,
        _ready: Any,
    ) -> Any: ...

    def molt_asyncio_timer_handle_cancel(
        _scheduled: Any, _handle: Any, _timer_task: Any | None
    ) -> None: ...

    def molt_asyncio_fd_watcher_new(
        _registry: Any, _fileno: Any, _callback: Any, _args: Any, _events: Any
    ) -> Any: ...

    def molt_asyncio_fd_watcher_register(
        _loop: Any,
        _registry: Any,
        _fileno: Any,
        _callback: Any,
        _args: Any,
        _events: Any,
    ) -> None: ...

    def molt_asyncio_fd_watcher_unregister(_registry: Any, _fileno: Any) -> bool: ...

    def molt_asyncio_subprocess_stdio_normalize(
        _value: Any,
        _allow_stdout: bool,
        _pipe_const: Any,
        _devnull_const: Any,
        _stdout_const: Any,
        _inherit_mode: int,
        _pipe_mode: int,
        _devnull_mode: int,
        _stdout_mode: int,
        _fd_base: int,
        _fd_max: int,
    ) -> int: ...

    def molt_asyncio_server_accept_loop_new(
        _sock: Any,
        _callback: Any,
        _loop: Any,
        _reader_ctor: Any,
        _writer_ctor: Any,
        _closed_probe: Any,
    ) -> Any: ...

    def molt_asyncio_ready_runner_new(
        _loop: Any, _ready_lock: Any, _ready: Any
    ) -> Any: ...

    def molt_asyncio_stream_reader_read_new(_reader: Any, _n: int = -1) -> Any: ...

    def molt_asyncio_stream_reader_readline_new(_reader: Any) -> Any: ...

    def molt_asyncio_stream_send_all_new(_stream: Any, _data: Any) -> Any: ...

    def molt_asyncio_stream_buffer_snapshot(_buffer: Any) -> Any: ...

    def molt_asyncio_stream_buffer_consume(_buffer: Any, _count: int) -> int: ...

    def molt_asyncio_socket_reader_read_new(
        _reader: Any, _n: int = -1, _fd: int = -1
    ) -> Any: ...

    def molt_asyncio_socket_reader_readline_new(_reader: Any, _fd: int = -1) -> Any: ...

    def molt_asyncio_sock_recv_new(_sock: Any, _size: int, _fd: int) -> Any: ...

    def molt_asyncio_sock_connect_new(_sock: Any, _address: Any, _fd: int) -> Any: ...

    def molt_asyncio_sock_accept_new(_sock: Any, _fd: int) -> Any: ...

    def molt_asyncio_sock_recv_into_new(
        _sock: Any, _buf: Any, _nbytes: int, _fd: int
    ) -> Any: ...

    def molt_asyncio_sock_sendall_new(_sock: Any, _data: Any, _fd: int) -> Any: ...

    def molt_asyncio_sock_recvfrom_new(_sock: Any, _size: int, _fd: int) -> Any: ...

    def molt_asyncio_sock_recvfrom_into_new(
        _sock: Any, _buf: Any, _nbytes: int, _fd: int
    ) -> Any: ...

    def molt_asyncio_sock_sendto_new(
        _sock: Any, _data: Any, _addr: Any, _fd: int
    ) -> Any: ...

    def molt_thread_submit(_func: Any, _args: Any, _kwargs: Any) -> Any: ...

    def molt_inspect_iscoroutine(_obj: Any) -> bool: ...

    def molt_inspect_iscoroutinefunction(_obj: Any) -> bool: ...


def _mark_builtin(fn: Any) -> None:
    func = _require_asyncio_intrinsic(
        _molt_function_set_builtin, "function_set_builtin"
    )
    func(fn)
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
    return bool(_molt_inspect_iscoroutine(obj))


def iscoroutinefunction(func: Any) -> bool:
    return bool(_molt_inspect_iscoroutinefunction(func))


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
        self._molt_promise: Any | None = molt_promise_new()
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
        promise = self._molt_promise
        if msg is None:
            _require_asyncio_intrinsic(molt_future_cancel, "future_cancel")(promise)
        else:
            _require_asyncio_intrinsic(_molt_future_cancel_msg, "future_cancel_msg")(
                promise, msg
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
            molt_promise_set_result(self._molt_promise, result)
        self._invoke_callbacks()

    def set_exception(self, exception: BaseException) -> None:
        if self._done:
            raise InvalidStateError("Result is already set")
        self._exception = exception
        if _is_cancelled_exc(exception):
            self._cancelled = True
        self._done = True
        if self._molt_promise is not None:
            molt_promise_set_exception(self._molt_promise, exception)
        self._invoke_callbacks()

    def _invoke_callbacks(self) -> None:
        callbacks = self._callbacks
        self._callbacks = []
        _require_asyncio_intrinsic(
            molt_asyncio_future_invoke_callbacks, "asyncio_future_invoke_callbacks"
        )(self, callbacks)

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
            await _async_yield_once()
        return self.result()

    def __await__(self) -> Any:
        async def _wrapped() -> Any:
            waiter = None
            if _EXPOSE_GRAPH:
                waiter = _task_registry_current()
                if isinstance(waiter, Future):
                    future_add_to_awaited_by(self, waiter)
            try:
                if _DEBUG_ASYNCIO_PROMISE:
                    _debug_write("asyncio_promise_await")
                return await self._molt_promise
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
_PROC_STDIO_INHERIT = 0
_PROC_STDIO_PIPE = 1
_PROC_STDIO_DEVNULL = 2
_PROC_STDIO_STDOUT = -2
_PROC_STDIO_FD_BASE = 1 << 30
_PROC_STDIO_FD_MAX = (1 << 31) - 1 - _PROC_STDIO_FD_BASE
_SUBPROCESS_PIPE = -1
_SUBPROCESS_STDOUT = -2
_SUBPROCESS_DEVNULL = -3
_WAIT_TIMEOUT_EPS = 1e-6


def _require_asyncio_intrinsic(
    fn: Callable[..., Any] | None, name: str
) -> Callable[..., Any]:
    if fn is None:
        raise RuntimeError(f"asyncio intrinsic not available: {name}")
    return fn


def _require_io_wait_new() -> Callable[..., Any]:
    if _molt_io_wait_new is None:
        raise RuntimeError("asyncio intrinsic not available: io_wait_new")
    return _molt_io_wait_new


def _require_ssl_transport_support(
    operation: str,
    ssl: Any,
    *,
    server_hostname: str | None = None,
    server_side: bool = False,
) -> bool:
    outcome = _require_asyncio_intrinsic(
        molt_asyncio_ssl_transport_orchestrate,
        "asyncio_ssl_transport_orchestrate",
    )(operation, ssl, server_hostname, server_side)
    if not isinstance(outcome, bool):
        raise RuntimeError("asyncio ssl transport intrinsic returned invalid payload")
    return outcome


def _tls_client_connect(
    host: str, port: int, server_hostname: str | None = None
) -> Any:
    return _require_asyncio_intrinsic(
        molt_asyncio_tls_client_connect_new,
        "asyncio_tls_client_connect_new",
    )(host, port, server_hostname)


def _tls_client_from_fd(fd: int, server_hostname: str | None = None) -> Any:
    return _require_asyncio_intrinsic(
        molt_asyncio_tls_client_from_fd_new,
        "asyncio_tls_client_from_fd_new",
    )(fd, server_hostname)


def _tls_server_payload(ssl: Any) -> tuple[str, str]:
    payload = _require_asyncio_intrinsic(
        molt_asyncio_tls_server_payload, "asyncio_tls_server_payload"
    )(ssl)
    if (
        isinstance(payload, tuple)
        and len(payload) == 2
        and isinstance(payload[0], str)
        and isinstance(payload[1], str)
    ):
        return payload
    raise RuntimeError("asyncio tls server payload intrinsic returned invalid payload")


def _tls_server_from_fd(fd: int, certfile: str, keyfile: str) -> Any:
    return _require_asyncio_intrinsic(
        molt_asyncio_tls_server_from_fd_new,
        "asyncio_tls_server_from_fd_new",
    )(fd, certfile, keyfile)


def _require_unix_socket_support() -> None:
    _require_asyncio_intrinsic(
        molt_asyncio_require_unix_socket_support,
        "asyncio_require_unix_socket_support",
    )()


def _require_child_watcher_support() -> None:
    _require_asyncio_intrinsic(
        molt_asyncio_require_child_watcher_support,
        "asyncio_require_child_watcher_support",
    )()


def _asyncio_cancel_pending_tasks(tasks: Any) -> int:
    return int(
        _require_asyncio_intrinsic(
            molt_asyncio_cancel_pending, "asyncio_cancel_pending"
        )(tasks)
    )


def _asyncio_waiters_notify(waiters: Any, count: int, result: Any) -> int:
    return int(
        _require_asyncio_intrinsic(
            molt_asyncio_waiters_notify, "asyncio_waiters_notify"
        )(waiters, count, result)
    )


def _asyncio_waiters_notify_exception(waiters: Any, count: int, exc: Any) -> int:
    return int(
        _require_asyncio_intrinsic(
            molt_asyncio_waiters_notify_exception, "asyncio_waiters_notify_exception"
        )(waiters, count, exc)
    )


def _asyncio_waiters_remove(waiters: Any, waiter: Any) -> bool:
    return bool(
        _require_asyncio_intrinsic(
            molt_asyncio_waiters_remove, "asyncio_waiters_remove"
        )(waiters, waiter)
    )


def _asyncio_barrier_release(waiters: Any) -> int:
    return int(
        _require_asyncio_intrinsic(
            molt_asyncio_barrier_release, "asyncio_barrier_release"
        )(waiters)
    )


def _asyncio_condition_wait_for_step(
    condition: Any, predicate: Callable[[], Any]
) -> tuple[bool, Any]:
    done, payload = _require_asyncio_intrinsic(
        molt_asyncio_condition_wait_for_step, "asyncio_condition_wait_for_step"
    )(condition, predicate)
    return bool(done), payload


def _asyncio_future_transfer(source: Any, target: Any) -> bool:
    return bool(
        _require_asyncio_intrinsic(
            molt_asyncio_future_transfer, "asyncio_future_transfer"
        )(source, target)
    )


def _asyncio_event_waiters_cleanup(waiters: Any) -> int:
    return int(
        _require_asyncio_intrinsic(
            molt_asyncio_event_waiters_cleanup, "asyncio_event_waiters_cleanup"
        )(waiters)
    )


def _task_registry_set(token_id: int, task: Any | None) -> None:
    _require_asyncio_intrinsic(
        molt_asyncio_task_registry_set, "asyncio_task_registry_set"
    )(token_id, task)


def _task_registry_get(token_id: int) -> Any | None:
    return _require_asyncio_intrinsic(
        molt_asyncio_task_registry_get, "asyncio_task_registry_get"
    )(token_id)


def _task_registry_contains(token_id: int) -> bool:
    return bool(
        _require_asyncio_intrinsic(
            molt_asyncio_task_registry_contains, "asyncio_task_registry_contains"
        )(token_id)
    )


def _task_registry_current() -> Any | None:
    return _require_asyncio_intrinsic(
        molt_asyncio_task_registry_current, "asyncio_task_registry_current"
    )()


def _task_registry_current_for_loop(loop: Any | None = None) -> Any | None:
    return _require_asyncio_intrinsic(
        molt_asyncio_task_registry_current_for_loop,
        "asyncio_task_registry_current_for_loop",
    )(loop)


def _task_registry_pop(token_id: int) -> Any | None:
    return _require_asyncio_intrinsic(
        molt_asyncio_task_registry_pop, "asyncio_task_registry_pop"
    )(token_id)


def _task_registry_move(old_token_id: int, new_token_id: int) -> bool:
    return bool(
        _require_asyncio_intrinsic(
            molt_asyncio_task_registry_move, "asyncio_task_registry_move"
        )(old_token_id, new_token_id)
    )


def _task_registry_values() -> Any:
    return _require_asyncio_intrinsic(
        molt_asyncio_task_registry_values, "asyncio_task_registry_values"
    )()


def _event_waiters_register(token_id: int, waiter: Any) -> None:
    _require_asyncio_intrinsic(
        molt_asyncio_event_waiters_register, "asyncio_event_waiters_register"
    )(token_id, waiter)


def _event_waiters_unregister(token_id: int, waiter: Any) -> bool:
    return bool(
        _require_asyncio_intrinsic(
            molt_asyncio_event_waiters_unregister, "asyncio_event_waiters_unregister"
        )(token_id, waiter)
    )


def _event_waiters_cleanup_token(token_id: int) -> int:
    return int(
        _require_asyncio_intrinsic(
            molt_asyncio_event_waiters_cleanup_token,
            "asyncio_event_waiters_cleanup_token",
        )(token_id)
    )


def _asyncio_ready_queue_drain(ready_lock: Any, ready: Any) -> int:
    return int(
        _require_asyncio_intrinsic(
            molt_asyncio_ready_queue_drain, "asyncio_ready_queue_drain"
        )(ready_lock, ready)
    )


def _asyncio_taskgroup_on_task_done(tasks: Any, errors: Any, task: Any) -> bool:
    return bool(
        _require_asyncio_intrinsic(
            molt_asyncio_taskgroup_on_task_done, "asyncio_taskgroup_on_task_done"
        )(tasks, errors, task)
    )


def _asyncio_tasks_add_done_callback(tasks: Any, callback: Callable[[Any], Any]) -> int:
    return int(
        _require_asyncio_intrinsic(
            molt_asyncio_tasks_add_done_callback, "asyncio_tasks_add_done_callback"
        )(tasks, callback)
    )


async def _async_yield_once() -> None:
    fut = molt_async_sleep(0.0, None)
    await fut


async def _io_wait(fd: int, events: int, timeout: float | None = None) -> Any:
    io_wait = _require_io_wait_new()
    return await _require_asyncio_intrinsic(io_wait, "io_wait_new")(fd, events, timeout)


_molt_io_wait_new = _intrinsic_require("molt_io_wait_new", globals())
molt_pending = _intrinsic_require("molt_pending", globals())
molt_async_sleep = _intrinsic_require("molt_async_sleep", globals())
molt_block_on = _intrinsic_require("molt_block_on", globals())
molt_asyncgen_shutdown = _intrinsic_require("molt_asyncgen_shutdown", globals())
molt_cancel_token_set_current = _intrinsic_require(
    "molt_cancel_token_set_current", globals()
)
molt_cancel_token_get_current = _intrinsic_require(
    "molt_cancel_token_get_current", globals()
)
molt_promise_new = _intrinsic_require("molt_promise_new", globals())
molt_promise_set_exception = _intrinsic_require("molt_promise_set_exception", globals())
molt_promise_set_result = _intrinsic_require("molt_promise_set_result", globals())
molt_task_register_token_owned = _intrinsic_require(
    "molt_task_register_token_owned", globals()
)
molt_future_cancel = _intrinsic_require("molt_future_cancel", globals())
molt_asyncio_wait_for_new = _intrinsic_require("molt_asyncio_wait_for_new", globals())
molt_asyncio_wait_new = _intrinsic_require("molt_asyncio_wait_new", globals())
molt_asyncio_gather_new = _intrinsic_require("molt_asyncio_gather_new", globals())
molt_asyncio_cancel_pending = _intrinsic_require(
    "molt_asyncio_cancel_pending", globals()
)
molt_asyncio_ready_batch_run = _intrinsic_require(
    "molt_asyncio_ready_batch_run", globals()
)
molt_asyncio_ready_queue_drain = _intrinsic_require(
    "molt_asyncio_ready_queue_drain", globals()
)
molt_asyncio_waiters_notify = _intrinsic_require(
    "molt_asyncio_waiters_notify", globals()
)
molt_asyncio_waiters_notify_exception = _intrinsic_require(
    "molt_asyncio_waiters_notify_exception", globals()
)
molt_asyncio_waiters_remove = _intrinsic_require(
    "molt_asyncio_waiters_remove", globals()
)
molt_asyncio_barrier_release = _intrinsic_require(
    "molt_asyncio_barrier_release", globals()
)
molt_asyncio_condition_wait_for_step = _intrinsic_require(
    "molt_asyncio_condition_wait_for_step", globals()
)
molt_asyncio_future_transfer = _intrinsic_require(
    "molt_asyncio_future_transfer", globals()
)
molt_asyncio_event_waiters_cleanup = _intrinsic_require(
    "molt_asyncio_event_waiters_cleanup", globals()
)
molt_asyncio_task_registry_set = _intrinsic_require(
    "molt_asyncio_task_registry_set", globals()
)
molt_asyncio_task_registry_get = _intrinsic_require(
    "molt_asyncio_task_registry_get", globals()
)
molt_asyncio_task_registry_contains = _intrinsic_require(
    "molt_asyncio_task_registry_contains", globals()
)
molt_asyncio_task_registry_current = _intrinsic_require(
    "molt_asyncio_task_registry_current", globals()
)
molt_asyncio_task_registry_current_for_loop = _intrinsic_require(
    "molt_asyncio_task_registry_current_for_loop", globals()
)
molt_asyncio_task_registry_pop = _intrinsic_require(
    "molt_asyncio_task_registry_pop", globals()
)
molt_asyncio_task_registry_move = _intrinsic_require(
    "molt_asyncio_task_registry_move", globals()
)
molt_asyncio_task_registry_values = _intrinsic_require(
    "molt_asyncio_task_registry_values", globals()
)
molt_asyncio_task_registry_live = _intrinsic_require(
    "molt_asyncio_task_registry_live", globals()
)
molt_asyncio_task_registry_live_set = _intrinsic_require(
    "molt_asyncio_task_registry_live_set", globals()
)
molt_asyncio_event_waiters_register = _intrinsic_require(
    "molt_asyncio_event_waiters_register", globals()
)
molt_asyncio_event_waiters_unregister = _intrinsic_require(
    "molt_asyncio_event_waiters_unregister", globals()
)
molt_asyncio_event_waiters_cleanup_token = _intrinsic_require(
    "molt_asyncio_event_waiters_cleanup_token", globals()
)
molt_asyncio_child_watcher_add = _intrinsic_require(
    "molt_asyncio_child_watcher_add", globals()
)
molt_asyncio_child_watcher_remove = _intrinsic_require(
    "molt_asyncio_child_watcher_remove", globals()
)
molt_asyncio_child_watcher_clear = _intrinsic_require(
    "molt_asyncio_child_watcher_clear", globals()
)
molt_asyncio_child_watcher_pop = _intrinsic_require(
    "molt_asyncio_child_watcher_pop", globals()
)
molt_asyncio_require_ssl_transport_support = _intrinsic_require(
    "molt_asyncio_require_ssl_transport_support", globals()
)
molt_asyncio_ssl_transport_orchestrate = _intrinsic_require(
    "molt_asyncio_ssl_transport_orchestrate", globals()
)
molt_asyncio_tls_client_connect_new = _intrinsic_require(
    "molt_asyncio_tls_client_connect_new", globals()
)
molt_asyncio_tls_client_from_fd_new = _intrinsic_require(
    "molt_asyncio_tls_client_from_fd_new", globals()
)
molt_asyncio_tls_server_payload = _intrinsic_require(
    "molt_asyncio_tls_server_payload", globals()
)
molt_asyncio_tls_server_from_fd_new = _intrinsic_require(
    "molt_asyncio_tls_server_from_fd_new", globals()
)
molt_asyncio_require_unix_socket_support = _intrinsic_require(
    "molt_asyncio_require_unix_socket_support", globals()
)
molt_asyncio_require_child_watcher_support = _intrinsic_require(
    "molt_asyncio_require_child_watcher_support", globals()
)
molt_asyncio_running_loop_get = _intrinsic_require(
    "molt_asyncio_running_loop_get", globals()
)
molt_asyncio_running_loop_set = _intrinsic_require(
    "molt_asyncio_running_loop_set", globals()
)
molt_asyncio_event_loop_get = _intrinsic_require(
    "molt_asyncio_event_loop_get", globals()
)
molt_asyncio_event_loop_set = _intrinsic_require(
    "molt_asyncio_event_loop_set", globals()
)
molt_asyncio_event_loop_policy_get = _intrinsic_require(
    "molt_asyncio_event_loop_policy_get", globals()
)
molt_asyncio_event_loop_policy_set = _intrinsic_require(
    "molt_asyncio_event_loop_policy_set", globals()
)
molt_asyncio_taskgroup_on_task_done = _intrinsic_require(
    "molt_asyncio_taskgroup_on_task_done", globals()
)
molt_asyncio_taskgroup_request_cancel = _intrinsic_require(
    "molt_asyncio_taskgroup_request_cancel", globals()
)
molt_asyncio_tasks_add_done_callback = _intrinsic_require(
    "molt_asyncio_tasks_add_done_callback", globals()
)
molt_asyncio_task_cancel_apply = _intrinsic_require(
    "molt_asyncio_task_cancel_apply", globals()
)
molt_asyncio_task_uncancel_apply = _intrinsic_require(
    "molt_asyncio_task_uncancel_apply", globals()
)
molt_asyncio_future_invoke_callbacks = _intrinsic_require(
    "molt_asyncio_future_invoke_callbacks", globals()
)
molt_asyncio_event_set_waiters = _intrinsic_require(
    "molt_asyncio_event_set_waiters", globals()
)
molt_asyncio_loop_enqueue_handle = _intrinsic_require(
    "molt_asyncio_loop_enqueue_handle", globals()
)
molt_asyncio_timer_handle_new = _intrinsic_require(
    "molt_asyncio_timer_handle_new", globals()
)
molt_asyncio_timer_schedule = _intrinsic_require(
    "molt_asyncio_timer_schedule", globals()
)
molt_asyncio_timer_handle_cancel = _intrinsic_require(
    "molt_asyncio_timer_handle_cancel", globals()
)
molt_asyncio_fd_watcher_new = _intrinsic_require(
    "molt_asyncio_fd_watcher_new", globals()
)
molt_asyncio_fd_watcher_register = _intrinsic_require(
    "molt_asyncio_fd_watcher_register", globals()
)
molt_asyncio_fd_watcher_unregister = _intrinsic_require(
    "molt_asyncio_fd_watcher_unregister", globals()
)
molt_asyncio_subprocess_stdio_normalize = _intrinsic_require(
    "molt_asyncio_subprocess_stdio_normalize", globals()
)
molt_asyncio_server_accept_loop_new = _intrinsic_require(
    "molt_asyncio_server_accept_loop_new", globals()
)
molt_asyncio_ready_runner_new = _intrinsic_require(
    "molt_asyncio_ready_runner_new", globals()
)
molt_asyncio_stream_reader_read_new = _intrinsic_require(
    "molt_asyncio_stream_reader_read_new", globals()
)
molt_asyncio_stream_reader_readline_new = _intrinsic_require(
    "molt_asyncio_stream_reader_readline_new", globals()
)
molt_asyncio_stream_send_all_new = _intrinsic_require(
    "molt_asyncio_stream_send_all_new", globals()
)
molt_asyncio_stream_buffer_snapshot = _intrinsic_require(
    "molt_asyncio_stream_buffer_snapshot", globals()
)
molt_asyncio_stream_buffer_consume = _intrinsic_require(
    "molt_asyncio_stream_buffer_consume", globals()
)
molt_asyncio_socket_reader_read_new = _intrinsic_require(
    "molt_asyncio_socket_reader_read_new", globals()
)
molt_asyncio_socket_reader_readline_new = _intrinsic_require(
    "molt_asyncio_socket_reader_readline_new", globals()
)
molt_asyncio_sock_recv_new = _intrinsic_require("molt_asyncio_sock_recv_new", globals())
molt_asyncio_sock_connect_new = _intrinsic_require(
    "molt_asyncio_sock_connect_new", globals()
)
molt_asyncio_sock_accept_new = _intrinsic_require(
    "molt_asyncio_sock_accept_new", globals()
)
molt_asyncio_sock_recv_into_new = _intrinsic_require(
    "molt_asyncio_sock_recv_into_new", globals()
)
molt_asyncio_sock_sendall_new = _intrinsic_require(
    "molt_asyncio_sock_sendall_new", globals()
)
molt_asyncio_sock_recvfrom_new = _intrinsic_require(
    "molt_asyncio_sock_recvfrom_new", globals()
)
molt_asyncio_sock_recvfrom_into_new = _intrinsic_require(
    "molt_asyncio_sock_recvfrom_into_new", globals()
)
molt_asyncio_sock_sendto_new = _intrinsic_require(
    "molt_asyncio_sock_sendto_new", globals()
)
molt_thread_submit = _intrinsic_require("molt_thread_submit", globals())

_molt_module_new = _intrinsic_require("molt_module_new", globals())
_molt_function_set_builtin = _intrinsic_require("molt_function_set_builtin", globals())
_molt_future_cancel_msg = _intrinsic_require("molt_future_cancel_msg", globals())
_molt_future_cancel_clear = _intrinsic_require("molt_future_cancel_clear", globals())
_molt_exception_pending = _intrinsic_require("molt_exception_pending", globals())
_molt_exception_last = _intrinsic_require("molt_exception_last", globals())
_molt_process_spawn = _intrinsic_require("molt_process_spawn", globals())
_molt_process_wait_future = _intrinsic_require("molt_process_wait_future", globals())
_molt_process_pid = _intrinsic_require("molt_process_pid", globals())
_molt_process_returncode = _intrinsic_require("molt_process_returncode", globals())
_molt_process_kill = _intrinsic_require("molt_process_kill", globals())
_molt_process_terminate = _intrinsic_require("molt_process_terminate", globals())
_molt_process_stdin = _intrinsic_require("molt_process_stdin", globals())
_molt_process_stdout = _intrinsic_require("molt_process_stdout", globals())
_molt_process_stderr = _intrinsic_require("molt_process_stderr", globals())
_molt_process_drop = _intrinsic_require("molt_process_drop", globals())
_molt_stream_new = _intrinsic_require("molt_stream_new", globals())
_molt_stream_recv = _intrinsic_require("molt_stream_recv", globals())
_molt_stream_send_obj = _intrinsic_require("molt_stream_send_obj", globals())
_molt_stream_close = _intrinsic_require("molt_stream_close", globals())
_molt_stream_drop = _intrinsic_require("molt_stream_drop", globals())
_molt_stream_reader_new = _intrinsic_require("molt_stream_reader_new", globals())
_molt_stream_reader_read = _intrinsic_require("molt_stream_reader_read", globals())
_molt_stream_reader_readline = _intrinsic_require(
    "molt_stream_reader_readline", globals()
)
_molt_stream_reader_at_eof = _intrinsic_require("molt_stream_reader_at_eof", globals())
_molt_stream_reader_drop = _intrinsic_require("molt_stream_reader_drop", globals())
_molt_socket_reader_new = _intrinsic_require("molt_socket_reader_new", globals())
_molt_socket_reader_read = _intrinsic_require("molt_socket_reader_read", globals())
_molt_socket_reader_readline = _intrinsic_require(
    "molt_socket_reader_readline", globals()
)
_molt_socket_reader_at_eof = _intrinsic_require("molt_socket_reader_at_eof", globals())
_molt_socket_reader_drop = _intrinsic_require("molt_socket_reader_drop", globals())
_molt_inspect_iscoroutine = _intrinsic_require("molt_inspect_iscoroutine", globals())
_molt_inspect_iscoroutinefunction = _intrinsic_require(
    "molt_inspect_iscoroutinefunction", globals()
)

_PENDING_SENTINEL: Any | None = None


def _pending_sentinel() -> Any:
    global _PENDING_SENTINEL
    if _PENDING_SENTINEL is None:
        _PENDING_SENTINEL = molt_pending()
    return _PENDING_SENTINEL


def _is_pending(value: Any) -> bool:
    pending = _pending_sentinel()
    return value is pending or value == pending


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
    PIPE = _SUBPROCESS_PIPE
    DEVNULL = _SUBPROCESS_DEVNULL
    STDOUT = _SUBPROCESS_STDOUT


subprocess = _SubprocessConstants


def _fd_from_fileobj(fileobj: Any) -> int:
    if isinstance(fileobj, int):
        return fileobj
    if hasattr(fileobj, "fileno"):
        return int(fileobj.fileno())
    raise ValueError("fileobj must be a file descriptor or have fileno()")


def _encode_proc_fd(fd: int) -> int:
    if fd < 0:
        raise ValueError("file descriptor must be >= 0")
    if fd > _PROC_STDIO_FD_MAX:
        raise ValueError("file descriptor is too large")
    return _PROC_STDIO_FD_BASE + int(fd)


def _normalize_proc_stdio(value: Any, *, allow_stdout: bool) -> int:
    return int(
        _require_asyncio_intrinsic(
            molt_asyncio_subprocess_stdio_normalize,
            "asyncio_subprocess_stdio_normalize",
        )(
            value,
            bool(allow_stdout),
            subprocess.PIPE,
            subprocess.DEVNULL,
            subprocess.STDOUT,
            _PROC_STDIO_INHERIT,
            _PROC_STDIO_PIPE,
            _PROC_STDIO_DEVNULL,
            _PROC_STDIO_STDOUT,
            _PROC_STDIO_FD_BASE,
            _PROC_STDIO_FD_MAX,
        )
    )


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
        _task_registry_set(self._token.token_id(), self)
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
        if _task_registry_get(old_id) is self:
            _task_registry_move(old_id, new_id)
        else:
            _task_registry_set(new_id, self)
        self._token = token
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
        _task_registry_pop(self._token.token_id())
        if extra_token_id is not None:
            _task_registry_pop(extra_token_id)
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
        _require_asyncio_intrinsic(
            molt_asyncio_event_set_waiters, "asyncio_event_set_waiters"
        )(waiters, True)
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
                _asyncio_waiters_remove(self._waiters, fut)
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
                _asyncio_waiters_remove(self._waiters, fut)
            raise
        self._locked = True
        return True

    def release(self) -> None:
        if not self._locked:
            raise RuntimeError("Lock is not acquired")
        if self._waiters:
            _asyncio_waiters_notify(self._waiters, 1, True)
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
                _asyncio_waiters_remove(self._waiters, fut)
            raise
        finally:
            await self.acquire()
        return True

    async def wait_for(self, predicate: Callable[[], bool]) -> bool:
        while True:
            done, payload = _asyncio_condition_wait_for_step(self, predicate)
            if done:
                return payload
            await payload

    def notify(self, n: int = 1) -> None:
        if not self.locked():
            raise RuntimeError("Condition lock is not acquired")
        _asyncio_waiters_notify(self._waiters, n, True)

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
                _asyncio_waiters_remove(self._waiters, fut)
            raise
        return True

    def release(self) -> None:
        if self._waiters:
            _asyncio_waiters_notify(self._waiters, 1, True)
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
            self._count = 0
            _asyncio_barrier_release(self._waiters)
        try:
            return await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                _asyncio_waiters_remove(self._waiters, fut)
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
        self._reader = _require_asyncio_intrinsic(
            _molt_socket_reader_new, "socket_reader_new"
        )(self._sock._require_handle())

    async def _wait_readable(self) -> None:
        if self._fd < 0:
            await sleep(0.0)
            return None
        _require_io_wait_new()
        await _io_wait(self._fd, 1)

    async def _read_raw(self, n: int) -> bytes:
        while True:
            try:
                res = await _require_asyncio_intrinsic(
                    molt_asyncio_socket_reader_read_new,
                    "asyncio_socket_reader_read_new",
                )(self._reader, n, self._fd)
            except (BlockingIOError, InterruptedError):
                await self._wait_readable()
                continue
            except OSError as exc:
                if exc.errno in (_errno.EAGAIN, _errno.EWOULDBLOCK):
                    await self._wait_readable()
                    continue
                raise
            if isinstance(res, (bytes, bytearray, memoryview)):
                self._eof = bool(
                    _require_asyncio_intrinsic(
                        _molt_socket_reader_at_eof, "socket_reader_at_eof"
                    )(self._reader)
                )
                return bytes(res)
            raise TypeError("socket stream reader returned non-bytes")

    def at_eof(self) -> bool:
        self._eof = bool(
            _require_asyncio_intrinsic(
                _molt_socket_reader_at_eof, "socket_reader_at_eof"
            )(self._reader)
        )
        return self._eof and not self._buffer

    async def read(self, n: int = -1) -> bytes:
        if n == 0:
            return b""
        if n < 0:
            chunks: list[bytes] = []
            if self._buffer:
                chunks.append(bytes(self._buffer))
                self._buffer.clear()
            data = await self._read_raw(-1)
            if data:
                chunks.append(data)
            return b"".join(chunks)
        if self._buffer:
            out = bytes(self._buffer[:n])
            del self._buffer[:n]
            return out
        return await self._read_raw(n)

    async def readexactly(self, n: int) -> bytes:
        if n <= 0:
            return b""
        out = bytearray()
        while len(out) < n:
            chunk = await self.read(n - len(out))
            if not chunk:
                raise IncompleteReadError(bytes(out), n)
            out.extend(chunk)
        return bytes(out)

    async def readuntil(self, separator: bytes = b"\n") -> bytes:
        sep = bytes(separator)
        if not sep:
            raise ValueError("Separator should be at least one-byte string")
        while True:
            idx = self._buffer.find(sep)
            if idx != -1:
                end = idx + len(sep)
                out = bytes(self._buffer[:end])
                del self._buffer[:end]
                return out
            if self._eof:
                partial = bytes(self._buffer)
                self._buffer.clear()
                raise IncompleteReadError(partial, len(sep))
            data = await self._read_raw(4096)
            if not data:
                self._eof = True
                continue
            self._buffer.extend(data)

    async def readline(self) -> bytes:
        if self._buffer:
            try:
                return await self.readuntil(b"\n")
            except IncompleteReadError as exc:
                return exc.partial
        while True:
            try:
                res = await _require_asyncio_intrinsic(
                    molt_asyncio_socket_reader_readline_new,
                    "asyncio_socket_reader_readline_new",
                )(self._reader, self._fd)
            except (BlockingIOError, InterruptedError):
                await self._wait_readable()
                continue
            except OSError as exc:
                if exc.errno in (_errno.EAGAIN, _errno.EWOULDBLOCK):
                    await self._wait_readable()
                    continue
                raise
            if isinstance(res, (bytes, bytearray, memoryview)):
                self._eof = bool(
                    _require_asyncio_intrinsic(
                        _molt_socket_reader_at_eof, "socket_reader_at_eof"
                    )(self._reader)
                )
                return bytes(res)
            raise TypeError("socket stream reader returned non-bytes")

    def __del__(self) -> None:
        reader = getattr(self, "_reader", None)
        if reader is None:
            return
        try:
            _require_asyncio_intrinsic(_molt_socket_reader_drop, "socket_reader_drop")(
                reader
            )
        except Exception:
            pass


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
        if not self._buffer:
            return None
        loop = get_running_loop()
        while self._buffer:
            chunk = _require_asyncio_intrinsic(
                molt_asyncio_stream_buffer_snapshot, "asyncio_stream_buffer_snapshot"
            )(self._buffer)
            if not chunk:
                return None
            await loop.sock_sendall(self._sock, chunk)
            _require_asyncio_intrinsic(
                molt_asyncio_stream_buffer_consume, "asyncio_stream_buffer_consume"
            )(self._buffer, len(chunk))

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
        raise RuntimeError("abstract asyncio server API")

    async def wait_closed(self) -> None:
        raise RuntimeError("abstract asyncio server API")

    def is_serving(self) -> bool:
        raise RuntimeError("abstract asyncio server API")

    async def start_serving(self) -> None:
        raise RuntimeError("abstract asyncio server API")

    async def serve_forever(self) -> None:
        raise RuntimeError("abstract asyncio server API")

    def get_loop(self) -> "EventLoop":
        raise RuntimeError("abstract asyncio server API")

    def close_clients(self) -> None:
        raise RuntimeError("abstract asyncio server API")

    def abort_clients(self) -> None:
        raise RuntimeError("abstract asyncio server API")


class Server(AbstractServer):
    def __init__(
        self,
        sock: _socket.socket,
        callback: Any,
        *,
        reader_ctor: Any = StreamReader,
        writer_ctor: Any = StreamWriter,
    ) -> None:
        self._sock = sock
        self._callback = callback
        self._reader_ctor = reader_ctor
        self._writer_ctor = writer_ctor
        self.sockets = [sock]
        self._closed = False
        self._accept_task = get_running_loop().create_task(
            self._accept_loop(), name=None, context=None
        )

    def _molt_is_closed(self) -> bool:
        return self._closed

    async def _accept_loop(self) -> None:
        loop = get_running_loop()
        await _require_asyncio_intrinsic(
            molt_asyncio_server_accept_loop_new, "asyncio_server_accept_loop_new"
        )(
            self._sock,
            self._callback,
            loop,
            self._reader_ctor,
            self._writer_ctor,
            self._molt_is_closed,
        )

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
        self._reader = _require_asyncio_intrinsic(
            _molt_stream_reader_new, "stream_reader_new"
        )(handle)

    def at_eof(self) -> bool:
        return bool(
            _require_asyncio_intrinsic(
                _molt_stream_reader_at_eof, "stream_reader_at_eof"
            )(self._reader)
        )

    async def read(self, n: int = -1) -> bytes:
        res = await _require_asyncio_intrinsic(
            molt_asyncio_stream_reader_read_new, "asyncio_stream_reader_read_new"
        )(self._reader, n)
        if isinstance(res, (bytes, bytearray, memoryview)):
            return bytes(res)
        raise TypeError("process stream reader returned non-bytes")

    async def readline(self) -> bytes:
        res = await _require_asyncio_intrinsic(
            molt_asyncio_stream_reader_readline_new,
            "asyncio_stream_reader_readline_new",
        )(self._reader)
        if isinstance(res, (bytes, bytearray, memoryview)):
            return bytes(res)
        raise TypeError("process stream reader returned non-bytes")

    async def readexactly(self, n: int) -> bytes:
        if n <= 0:
            return b""
        buf = bytearray()
        while len(buf) < n:
            chunk = await self.read(n - len(buf))
            if not chunk:
                raise IncompleteReadError(bytes(buf), n)
            buf.extend(chunk)
        return bytes(buf)

    def __del__(self) -> None:
        reader = getattr(self, "_reader", None)
        if reader is None:
            return
        try:
            _require_asyncio_intrinsic(_molt_stream_reader_drop, "stream_reader_drop")(
                reader
            )
        except Exception:
            pass


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
            chunk = _require_asyncio_intrinsic(
                molt_asyncio_stream_buffer_snapshot, "asyncio_stream_buffer_snapshot"
            )(self._buffer)
            if not chunk:
                return None
            await _require_asyncio_intrinsic(
                molt_asyncio_stream_send_all_new, "asyncio_stream_send_all_new"
            )(self._handle, chunk)
            _require_asyncio_intrinsic(
                molt_asyncio_stream_buffer_consume, "asyncio_stream_buffer_consume"
            )(self._buffer, len(chunk))

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

    async def communicate(
        self, input: bytes | None = None
    ) -> tuple[bytes | None, bytes | None]:
        if input is not None:
            if self.stdin is None:
                raise ValueError("stdin was not set to PIPE")
            self.stdin.write(input)
            await self.stdin.drain()
            self.stdin.close()

        tasks: list[tuple[str, Future]] = []
        if self.stdout is not None:
            tasks.append(("stdout", ensure_future(self.stdout.read())))
        if self.stderr is not None:
            tasks.append(("stderr", ensure_future(self.stderr.read())))

        out: bytes | None = None
        err: bytes | None = None
        try:
            if tasks:
                results = await gather(*[task for _, task in tasks])
                for idx, result in enumerate(results):
                    kind, _task = tasks[idx]
                    if kind == "stdout":
                        out = result
                    else:
                        err = result
            await self.wait()
        except BaseException:
            for _, task in tasks:
                try:
                    task.cancel()
                except Exception:
                    pass
            raise
        return out, err

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
        _require_asyncio_intrinsic(
            molt_asyncio_timer_handle_cancel, "asyncio_timer_handle_cancel"
        )(
            self._loop._scheduled,  # type: ignore[attr-defined]
            self,
            self._timer_task,
        )
        self._timer_task = None


class AbstractEventLoop:
    def run_forever(self) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    def run_until_complete(self, future: Any) -> Any:
        raise RuntimeError("abstract asyncio event loop API")

    def stop(self) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    def is_running(self) -> bool:
        raise RuntimeError("abstract asyncio event loop API")

    def is_closed(self) -> bool:
        raise RuntimeError("abstract asyncio event loop API")

    def close(self) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    async def shutdown_asyncgens(self) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    async def shutdown_default_executor(self) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    def create_task(
        self, coro: Any, *, name: str | None = None, context: Any | None = None
    ) -> Task:
        raise RuntimeError("abstract asyncio event loop API")

    def set_task_factory(self, factory: Callable[..., Task] | None) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    def get_task_factory(self) -> Callable[..., Task] | None:
        raise RuntimeError("abstract asyncio event loop API")

    def create_future(self) -> Future:
        raise RuntimeError("abstract asyncio event loop API")

    def call_soon(
        self, callback: Callable[..., Any], /, *args: Any, context: Any | None = None
    ) -> Handle:
        raise RuntimeError("abstract asyncio event loop API")

    def call_soon_threadsafe(
        self, callback: Callable[..., Any], /, *args: Any, context: Any | None = None
    ) -> Handle:
        raise RuntimeError("abstract asyncio event loop API")

    def call_later(
        self,
        delay: float,
        callback: Callable[..., Any],
        /,
        *args: Any,
        context: Any | None = None,
    ) -> TimerHandle:
        raise RuntimeError("abstract asyncio event loop API")

    def call_at(
        self,
        when: float,
        callback: Callable[..., Any],
        /,
        *args: Any,
        context: Any | None = None,
    ) -> TimerHandle:
        raise RuntimeError("abstract asyncio event loop API")

    def time(self) -> float:
        raise RuntimeError("abstract asyncio event loop API")

    def call_exception_handler(self, context: dict[str, Any]) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    def default_exception_handler(self, context: dict[str, Any]) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    def set_exception_handler(
        self, handler: Callable[["AbstractEventLoop", dict[str, Any]], Any] | None
    ) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    def get_exception_handler(
        self,
    ) -> Callable[["AbstractEventLoop", dict[str, Any]], Any] | None:
        raise RuntimeError("abstract asyncio event loop API")

    def get_debug(self) -> bool:
        raise RuntimeError("abstract asyncio event loop API")

    def set_debug(self, enabled: bool) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    def add_signal_handler(
        self, sig: int, callback: Callable[..., Any], /, *args: Any
    ) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    def remove_signal_handler(self, sig: int) -> bool:
        raise RuntimeError("abstract asyncio event loop API")

    def add_reader(self, fd: int, callback: Callable[..., Any], /, *args: Any) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    def remove_reader(self, fd: int) -> bool:
        raise RuntimeError("abstract asyncio event loop API")

    def add_writer(self, fd: int, callback: Callable[..., Any], /, *args: Any) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    def remove_writer(self, fd: int) -> bool:
        raise RuntimeError("abstract asyncio event loop API")

    async def create_connection(
        self,
        protocol_factory: Callable[[], Protocol] | None,
        host: str | None = None,
        port: int | None = None,
        /,
        **kwargs: Any,
    ) -> tuple[Transport, Protocol]:
        raise RuntimeError("abstract asyncio event loop API")

    async def create_server(
        self,
        protocol_factory: Callable[[], Protocol],
        host: str | None = None,
        port: int | None = None,
        /,
        **kwargs: Any,
    ) -> AbstractServer:
        raise RuntimeError("abstract asyncio event loop API")

    async def create_datagram_endpoint(
        self,
        protocol_factory: Callable[[], DatagramProtocol],
        local_addr: Any | None = None,
        remote_addr: Any | None = None,
        /,
        **kwargs: Any,
    ) -> tuple[DatagramTransport, DatagramProtocol]:
        raise RuntimeError("abstract asyncio event loop API")

    async def connect_accepted_socket(
        self,
        protocol_factory: Callable[[], Protocol],
        sock: _socket.socket,
        /,
        **kwargs,
    ) -> tuple[Transport, Protocol]:
        raise RuntimeError("abstract asyncio event loop API")

    async def create_unix_connection(
        self,
        protocol_factory: Callable[[], Protocol],
        path: str | None = None,
        /,
        **kwargs: Any,
    ) -> tuple[Transport, Protocol]:
        raise RuntimeError("abstract asyncio event loop API")

    async def create_unix_server(
        self,
        protocol_factory: Callable[[], Protocol],
        path: str | None = None,
        /,
        **kwargs: Any,
    ) -> AbstractServer:
        raise RuntimeError("abstract asyncio event loop API")

    async def create_subprocess_shell(self, protocol_factory: Any, cmd: Any, **kwargs):
        raise RuntimeError("abstract asyncio event loop API")

    async def create_subprocess_exec(self, protocol_factory: Any, *args: Any, **kwargs):
        raise RuntimeError("abstract asyncio event loop API")

    async def start_tls(
        self,
        transport: Transport,
        protocol: Protocol,
        sslcontext: Any,
        *,
        server_side: bool = False,
        server_hostname: str | None = None,
        ssl_handshake_timeout: float | None = None,
        ssl_shutdown_timeout: float | None = None,
    ):
        raise RuntimeError("abstract asyncio event loop API")

    async def sendfile(self, transport: Transport, file: Any, **kwargs: Any) -> Any:
        raise RuntimeError("abstract asyncio event loop API")

    def set_default_executor(self, executor: Any) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    def run_in_executor(self, executor: Any, func: Any, *args: Any) -> Future:
        raise RuntimeError("abstract asyncio event loop API")

    async def getaddrinfo(self, host: Any, port: Any, **kwargs: Any) -> Any:
        raise RuntimeError("abstract asyncio event loop API")

    async def getnameinfo(self, sockaddr: Any, flags: int) -> Any:
        raise RuntimeError("abstract asyncio event loop API")

    async def sock_recv(self, sock: Any, n: int) -> bytes:
        raise RuntimeError("abstract asyncio event loop API")

    async def sock_recv_into(self, sock: Any, buf: Any) -> int:
        raise RuntimeError("abstract asyncio event loop API")

    async def sock_recvfrom(self, sock: Any, bufsize: int) -> tuple[Any, Any]:
        raise RuntimeError("abstract asyncio event loop API")

    async def sock_recvfrom_into(self, sock: Any, buf: Any) -> tuple[int, Any]:
        raise RuntimeError("abstract asyncio event loop API")

    async def sock_sendall(self, sock: Any, data: bytes) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    async def sock_sendto(self, sock: Any, data: bytes, addr: Any) -> int:
        raise RuntimeError("abstract asyncio event loop API")

    async def sock_connect(self, sock: Any, address: Any) -> None:
        raise RuntimeError("abstract asyncio event loop API")

    async def sock_accept(self, sock: Any) -> tuple[Any, Any]:
        raise RuntimeError("abstract asyncio event loop API")

    async def sock_sendfile(self, sock: Any, file: Any, offset: int = 0, count=None):
        raise RuntimeError("abstract asyncio event loop API")


class _EventLoop(AbstractEventLoop):
    def __init__(self, selector: Any | None = None) -> None:
        self._readers: dict[int, tuple[Any, tuple[Any, ...], Task]] = {}
        self._writers: dict[int, tuple[Any, tuple[Any, ...], Task]] = {}
        self._ready: _deque[Handle] = _deque()
        self._ready_lock = _threading.Lock()
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
        self._default_executor: Any | None = None
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
        runner = _require_asyncio_intrinsic(
            molt_asyncio_ready_runner_new, "asyncio_ready_runner_new"
        )(self, self._ready_lock, self._ready)
        self._ready_task = self.create_task(runner, name=None, context=None)

    async def _ready_loop(self) -> None:
        runner = _require_asyncio_intrinsic(
            molt_asyncio_ready_runner_new, "asyncio_ready_runner_new"
        )(self, self._ready_lock, self._ready)
        await runner

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
        _require_asyncio_intrinsic(
            molt_asyncio_loop_enqueue_handle, "asyncio_loop_enqueue_handle"
        )(self, self._ready_lock, self._ready, handle)
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
        timer_task = _require_asyncio_intrinsic(
            molt_asyncio_timer_schedule, "asyncio_timer_schedule"
        )(
            handle,
            delay,
            self,
            self._scheduled,
            self._ready_lock,
            self._ready,
        )
        if timer_task is not None:
            handle._timer_task = timer_task
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
        timer_task = _require_asyncio_intrinsic(
            molt_asyncio_timer_schedule, "asyncio_timer_schedule"
        )(
            handle,
            delay,
            self,
            self._scheduled,
            self._ready_lock,
            self._ready,
        )
        if timer_task is not None:
            handle._timer_task = timer_task
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
        if executor is None:
            executor = self._default_executor
        if executor is None:
            future = molt_thread_submit(func, args, {})
            return future
        submit = getattr(executor, "submit", None)
        if submit is None or not callable(submit):
            raise TypeError("executor must define submit()")
        try:
            submitted = submit(func, *args)
        except BaseException as exc:
            failed = Future()
            failed.set_exception(exc)
            return failed
        return wrap_future(submitted, loop=self)

    def add_reader(self, fd: Any, callback: Any, *args: Any) -> None:
        fileno = _fd_from_fileobj(fd)
        _require_asyncio_intrinsic(
            molt_asyncio_fd_watcher_register, "asyncio_fd_watcher_register"
        )(self, self._readers, fileno, callback, args, 1)

    def remove_reader(self, fd: Any) -> bool:
        fileno = _fd_from_fileobj(fd)
        return bool(
            _require_asyncio_intrinsic(
                molt_asyncio_fd_watcher_unregister, "asyncio_fd_watcher_unregister"
            )(self._readers, fileno)
        )

    def add_writer(self, fd: Any, callback: Any, *args: Any) -> None:
        fileno = _fd_from_fileobj(fd)
        _require_asyncio_intrinsic(
            molt_asyncio_fd_watcher_register, "asyncio_fd_watcher_register"
        )(self, self._writers, fileno, callback, args, 2)

    async def sock_recv(self, sock: Any, n: int) -> bytes:
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_recv_new, "asyncio_sock_recv_new"
        )(sock, n, sock.fileno())
        return await fut

    async def sock_recv_into(self, sock: Any, buf: Any) -> int:
        nbytes = len(buf)
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_recv_into_new, "asyncio_sock_recv_into_new"
        )(sock, buf, nbytes, sock.fileno())
        return await fut

    async def sock_sendall(self, sock: Any, data: bytes) -> None:
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_sendall_new, "asyncio_sock_sendall_new"
        )(sock, data, sock.fileno())
        await fut

    async def sock_recvfrom(self, sock: Any, bufsize: int) -> tuple[Any, Any]:
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_recvfrom_new, "asyncio_sock_recvfrom_new"
        )(sock, bufsize, sock.fileno())
        return await fut

    async def sock_recvfrom_into(self, sock: Any, buf: Any) -> tuple[int, Any]:
        nbytes = len(buf)
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_recvfrom_into_new, "asyncio_sock_recvfrom_into_new"
        )(sock, buf, nbytes, sock.fileno())
        return await fut

    async def sock_sendto(self, sock: Any, data: bytes, addr: Any) -> int:
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_sendto_new, "asyncio_sock_sendto_new"
        )(sock, data, addr, sock.fileno())
        return await fut

    async def sock_connect(self, sock: Any, address: Any) -> None:
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_connect_new, "asyncio_sock_connect_new"
        )(sock, address, sock.fileno())
        await fut

    async def sock_accept(self, sock: Any) -> tuple[Any, Any]:
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_accept_new, "asyncio_sock_accept_new"
        )(sock, sock.fileno())
        return await fut

    def remove_writer(self, fd: Any) -> bool:
        fileno = _fd_from_fileobj(fd)
        return bool(
            _require_asyncio_intrinsic(
                molt_asyncio_fd_watcher_unregister, "asyncio_fd_watcher_unregister"
            )(self._writers, fileno)
        )

    def run_until_complete(self, future: Any) -> Any:
        if self._closed:
            raise RuntimeError("Event loop is closed")
        if self._running:
            raise RuntimeError("Event loop is already running")
        prev = _get_running_loop()
        _set_running_loop(self)
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
            _set_running_loop(prev)
        _debug_exc_state("run_until_complete_return")
        return result

    def run_forever(self) -> None:
        async def _spin() -> None:
            while not self._stopping:
                await sleep(0.0)

        self.run_until_complete(_spin())

    async def shutdown_asyncgens(self) -> None:
        _require_asyncio_intrinsic(molt_asyncgen_shutdown, "asyncgen_shutdown")()
        return None

    async def shutdown_default_executor(self) -> None:
        self._default_executor = None
        return None

    def set_default_executor(self, executor: Any) -> None:
        if executor is not None:
            submit = getattr(executor, "submit", None)
            if submit is None or not callable(submit):
                raise TypeError("executor must define submit()")
        self._default_executor = executor

    async def create_connection(
        self,
        protocol_factory: Callable[[], Protocol] | None,
        host: str | None = None,
        port: int | None = None,
        /,
        **kwargs: Any,
    ) -> tuple[Transport, Protocol]:
        if protocol_factory is None:
            raise TypeError("protocol_factory must be callable")
        ssl = kwargs.pop("ssl", None)
        local_addr = kwargs.pop("local_addr", None)
        if kwargs:
            raise TypeError("unsupported create_connection options")
        if host is None or port is None:
            raise TypeError("host and port are required")
        reader, writer = await open_connection(
            host, int(port), ssl=ssl, local_addr=local_addr
        )
        protocol = protocol_factory()
        connection_made = getattr(protocol, "connection_made", None)
        if callable(connection_made):
            connection_made(writer)
        return writer, protocol

    async def create_server(
        self,
        protocol_factory: Callable[[], Protocol],
        host: str | None = None,
        port: int | None = None,
        /,
        **kwargs: Any,
    ) -> AbstractServer:
        ssl = kwargs.pop("ssl", None)
        backlog = int(kwargs.pop("backlog", 100))
        reuse_port = bool(kwargs.pop("reuse_port", False))
        if kwargs:
            raise TypeError("unsupported create_server options")

        async def _on_client(reader: StreamReader, writer: StreamWriter) -> None:
            protocol = protocol_factory()
            connection_made = getattr(protocol, "connection_made", None)
            if callable(connection_made):
                connection_made(writer)
            client_connected = getattr(protocol, "client_connected_cb", None)
            if callable(client_connected):
                maybe = client_connected(reader, writer)
                if hasattr(maybe, "__await__"):
                    await maybe

        return await start_server(
            _on_client,
            host=host,
            port=port,
            backlog=backlog,
            reuse_port=reuse_port,
            ssl=ssl,
        )

    async def create_datagram_endpoint(
        self,
        protocol_factory: Callable[[], DatagramProtocol],
        local_addr: Any | None = None,
        remote_addr: Any | None = None,
        /,
        **kwargs: Any,
    ) -> tuple[DatagramTransport, DatagramProtocol]:
        family = int(kwargs.pop("family", 0) or 0)
        proto = int(kwargs.pop("proto", 0) or 0)
        reuse_port = bool(kwargs.pop("reuse_port", False))
        if kwargs:
            raise TypeError("unsupported create_datagram_endpoint options")
        if local_addr is None and remote_addr is None:
            raise ValueError("local_addr or remote_addr must be specified")
        if family == 0:
            family = _socket.AF_INET
        sock = _socket.socket(family, _socket.SOCK_DGRAM, proto)
        sock.setblocking(False)
        if reuse_port and hasattr(_socket, "SO_REUSEPORT"):
            sock.setsockopt(
                _socket.SOL_SOCKET, int(getattr(_socket, "SO_REUSEPORT")), 1
            )
        if local_addr is not None:
            sock.bind(local_addr)
        if remote_addr is not None:
            await self.sock_connect(sock, remote_addr)
        transport = _DatagramSocketTransport(sock, self)
        protocol = protocol_factory()
        connection_made = getattr(protocol, "connection_made", None)
        if callable(connection_made):
            connection_made(transport)
        return transport, protocol

    async def connect_accepted_socket(
        self,
        protocol_factory: Callable[[], Protocol],
        sock: _socket.socket,
        /,
        **kwargs,
    ) -> tuple[Transport, Protocol]:
        if kwargs:
            raise TypeError("unsupported connect_accepted_socket options")
        sock.setblocking(False)
        writer = StreamWriter(sock)
        protocol = protocol_factory()
        connection_made = getattr(protocol, "connection_made", None)
        if callable(connection_made):
            connection_made(writer)
        return writer, protocol

    async def create_unix_connection(
        self,
        protocol_factory: Callable[[], Protocol],
        path: str | None = None,
        /,
        **kwargs: Any,
    ) -> tuple[Transport, Protocol]:
        if protocol_factory is None:
            raise TypeError("protocol_factory must be callable")
        if path is None:
            raise TypeError("path is required")
        ssl = kwargs.pop("ssl", None)
        local_addr = kwargs.pop("local_addr", None)
        if kwargs:
            raise TypeError("unsupported create_unix_connection options")
        reader, writer = await open_unix_connection(
            path, ssl=ssl, local_addr=local_addr
        )
        protocol = protocol_factory()
        connection_made = getattr(protocol, "connection_made", None)
        if callable(connection_made):
            connection_made(writer)
        return writer, protocol

    async def create_unix_server(
        self,
        protocol_factory: Callable[[], Protocol],
        path: str | None = None,
        /,
        **kwargs: Any,
    ) -> AbstractServer:
        if path is None:
            raise TypeError("path is required")
        ssl = kwargs.pop("ssl", None)
        backlog = int(kwargs.pop("backlog", 100))
        if kwargs:
            raise TypeError("unsupported create_unix_server options")

        async def _on_client(reader: StreamReader, writer: StreamWriter) -> None:
            protocol = protocol_factory()
            connection_made = getattr(protocol, "connection_made", None)
            if callable(connection_made):
                connection_made(writer)

        return await start_unix_server(_on_client, path, backlog=backlog, ssl=ssl)

    async def create_subprocess_shell(self, protocol_factory: Any, cmd: Any, **kwargs):
        process = await create_subprocess_shell(cmd, **kwargs)
        protocol = protocol_factory()
        connection_made = getattr(protocol, "connection_made", None)
        if callable(connection_made):
            connection_made(process)
        return process, protocol

    async def create_subprocess_exec(self, protocol_factory: Any, *args: Any, **kwargs):
        process = await create_subprocess_exec(*args, **kwargs)
        protocol = protocol_factory()
        connection_made = getattr(protocol, "connection_made", None)
        if callable(connection_made):
            connection_made(process)
        return process, protocol

    async def start_tls(
        self,
        transport: Transport,
        protocol: Protocol,
        sslcontext: Any,
        *,
        server_side: bool = False,
        server_hostname: str | None = None,
        ssl_handshake_timeout: float | None = None,
        ssl_shutdown_timeout: float | None = None,
    ):
        # Handshake/shutdown timeout knobs are part of the public API surface.
        # The runtime TLS lane owns execution and timeout handling semantics.
        _ = (ssl_handshake_timeout, ssl_shutdown_timeout)
        use_tls = _require_ssl_transport_support(
            "start_tls",
            sslcontext,
            server_hostname=None if server_side else server_hostname,
            server_side=server_side,
        )
        if not use_tls:
            return transport
        sock = getattr(transport, "_sock", None)
        if sock is None or not hasattr(sock, "detach"):
            raise TypeError("start_tls currently requires a stream socket transport")
        resolved_server_hostname = server_hostname
        if not server_side and resolved_server_hostname is None:
            try:
                peer = sock.getpeername()
                if isinstance(peer, tuple) and peer:
                    host = peer[0]
                    if isinstance(host, str) and host:
                        resolved_server_hostname = host
            except Exception:
                resolved_server_hostname = None
        raw_fd = sock.detach()
        if not isinstance(raw_fd, int) or raw_fd < 0:
            raise OSError("start_tls could not detach transport socket")
        if server_side:
            certfile, keyfile = _tls_server_payload(sslcontext)
            upgraded = ProcessStreamWriter(
                _tls_server_from_fd(raw_fd, certfile, keyfile)
            )
        else:
            upgraded = ProcessStreamWriter(
                _tls_client_from_fd(raw_fd, resolved_server_hostname)
            )
        if hasattr(transport, "_closed"):
            try:
                transport._closed = True
            except Exception:
                pass
        connection_made = getattr(protocol, "connection_made", None)
        if callable(connection_made):
            connection_made(upgraded)
        return upgraded

    async def sendfile(self, transport: Transport, file: Any, **kwargs: Any) -> Any:
        sock = getattr(transport, "_sock", None)
        if sock is None and isinstance(transport, StreamWriter):
            sock = getattr(transport, "_sock", None)
        if sock is None:
            raise RuntimeError("transport does not expose an underlying socket")
        offset = int(kwargs.get("offset", 0) or 0)
        count = kwargs.get("count")
        return await self.sock_sendfile(sock, file, offset=offset, count=count)

    async def getaddrinfo(self, host: Any, port: Any, **kwargs: Any) -> Any:
        return _socket.getaddrinfo(host, port, **kwargs)

    async def getnameinfo(self, sockaddr: Any, flags: int) -> Any:
        return _socket.getnameinfo(sockaddr, flags)

    async def sock_sendfile(self, sock: Any, file: Any, offset: int = 0, count=None):
        chunk_size = 256 * 1024
        if offset:
            file.seek(offset)
        remaining = None if count is None else max(0, int(count))
        sent = 0
        while remaining is None or remaining > 0:
            to_read = chunk_size if remaining is None else min(chunk_size, remaining)
            chunk = file.read(to_read)
            if not chunk:
                break
            await self.sock_sendall(sock, chunk)
            sent += len(chunk)
            if remaining is not None:
                remaining -= len(chunk)
        return sent


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
        raise RuntimeError("abstract asyncio event loop policy API")

    def set_event_loop(self, loop: EventLoop | None) -> None:
        raise RuntimeError("abstract asyncio event loop policy API")

    def new_event_loop(self) -> EventLoop:
        raise RuntimeError("abstract asyncio event loop policy API")


class DefaultEventLoopPolicy(AbstractEventLoopPolicy):
    def get_event_loop(self) -> EventLoop:
        loop = molt_asyncio_event_loop_get()
        if loop is None:
            loop = _EventLoop()
            molt_asyncio_event_loop_set(loop)
        return loop

    def set_event_loop(self, loop: EventLoop | None) -> None:
        molt_asyncio_event_loop_set(loop)

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
        _require_asyncio_intrinsic(
            molt_asyncio_child_watcher_add, "asyncio_child_watcher_add"
        )(self._callbacks, int(pid), callback, args)

    def remove_child_handler(self, pid: int) -> bool:
        return bool(
            _require_asyncio_intrinsic(
                molt_asyncio_child_watcher_remove, "asyncio_child_watcher_remove"
            )(self._callbacks, int(pid))
        )

    def close(self) -> None:
        _require_asyncio_intrinsic(
            molt_asyncio_child_watcher_clear, "asyncio_child_watcher_clear"
        )(self._callbacks)
        self._loop = None

    def is_active(self) -> bool:
        return self._loop is not None

    def _notify_child_exit(self, pid: int, returncode: int) -> None:
        entry = _require_asyncio_intrinsic(
            molt_asyncio_child_watcher_pop, "asyncio_child_watcher_pop"
        )(self._callbacks, int(pid))
        if entry is None:
            return
        if (
            not isinstance(entry, (tuple, list))
            or len(entry) != 2
            or not isinstance(entry[1], (tuple, list))
        ):
            raise RuntimeError(
                "asyncio child_watcher_pop intrinsic returned invalid value"
            )
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
    _require_child_watcher_support()
    global _CHILD_WATCHER
    if _CHILD_WATCHER is None:
        _CHILD_WATCHER = SafeChildWatcher()
    loop = _get_running_loop()
    if loop is not None:
        _CHILD_WATCHER.attach_loop(loop)
    return _CHILD_WATCHER


def set_child_watcher(watcher: AbstractChildWatcher | None) -> None:
    _require_child_watcher_support()
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


class _DatagramSocketTransport(DatagramTransport):
    def __init__(self, sock: _socket.socket, loop: "_EventLoop") -> None:
        self._sock = sock
        self._loop = loop
        self._closed = False

    def sendto(self, data: bytes, addr: Any | None = None) -> int:
        if self._closed:
            raise RuntimeError("transport is closed")
        if addr is None:
            return self._sock.send(data)
        return self._sock.sendto(data, addr)

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        try:
            self._sock.close()
        except Exception:
            pass

    def is_closing(self) -> bool:
        return self._closed

    def get_extra_info(self, name: str, default: Any = None) -> Any:
        if name == "socket":
            return self._sock
        return default


def _get_running_loop() -> EventLoop | None:
    return molt_asyncio_running_loop_get()


def _set_running_loop(loop: EventLoop | None) -> None:
    molt_asyncio_running_loop_set(loop)


def get_running_loop() -> EventLoop:
    loop = _get_running_loop()
    if loop is None:
        raise RuntimeError("no running event loop")
    return loop


def get_event_loop_policy() -> AbstractEventLoopPolicy:
    policy = molt_asyncio_event_loop_policy_get()
    if policy is None:
        policy = _default_event_loop_policy()
        molt_asyncio_event_loop_policy_set(policy)
    return policy


def set_event_loop_policy(policy: AbstractEventLoopPolicy | None) -> None:
    if policy is None:
        policy = _default_event_loop_policy()
    molt_asyncio_event_loop_policy_set(policy)


def get_event_loop() -> EventLoop:
    return get_event_loop_policy().get_event_loop()


def set_event_loop(loop: EventLoop | None) -> None:
    get_event_loop_policy().set_event_loop(loop)


def new_event_loop() -> EventLoop:
    return get_event_loop_policy().new_event_loop()


def _cancel_all_tasks(loop: EventLoop) -> None:
    try:
        live_tasks = all_tasks(loop)
    except BaseException:
        return
    tasks = list(live_tasks)
    if not tasks:
        return
    _asyncio_cancel_pending_tasks(tasks)
    try:
        waiter = _require_asyncio_intrinsic(
            molt_asyncio_gather_new, "asyncio_gather_new"
        )(tasks, True)
        loop.run_until_complete(waiter)
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
    if _get_running_loop() is not None:
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
        delay = 0.0
    else:
        delay = float(delay)
    fut = _require_asyncio_intrinsic(molt_async_sleep, "async_sleep")(delay, result)
    return await fut


async def open_connection(
    host: str,
    port: int,
    *,
    ssl: Any | None = None,
    local_addr: Any | None = None,
) -> tuple["StreamReader", "StreamWriter"]:
    if ssl is not None:
        use_tls = _require_ssl_transport_support(
            "open_connection",
            ssl,
            server_hostname=host if ssl is not False else None,
            server_side=False,
        )
        if use_tls:
            tls_handle = _tls_client_connect(
                host,
                int(port),
                host if ssl is not False else None,
            )
            return (
                ProcessStreamReader(tls_handle),
                ProcessStreamWriter(tls_handle),
            )
    sock = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM)
    if local_addr is not None:
        sock.bind(local_addr)
    sock.setblocking(False)
    loop = get_running_loop()
    await loop.sock_connect(sock, (host, port))
    reader = StreamReader(sock)
    writer = StreamWriter(sock)
    return reader, writer


async def open_unix_connection(
    path: str,
    *,
    ssl: Any | None = None,
    local_addr: Any | None = None,
) -> tuple["StreamReader", "StreamWriter"]:
    _require_unix_socket_support()
    use_tls = False
    if ssl is not None:
        use_tls = _require_ssl_transport_support(
            "open_unix_connection", ssl, server_side=False
        )
    sock = _socket.socket(_socket.AF_UNIX, _socket.SOCK_STREAM)
    if local_addr is not None:
        sock.bind(local_addr)
    sock.setblocking(False)
    loop = get_running_loop()
    await loop.sock_connect(sock, path)
    if use_tls:
        raw_fd = sock.detach()
        handle = _tls_client_from_fd(raw_fd, None)
        return (
            ProcessStreamReader(handle),
            ProcessStreamWriter(handle),
        )
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
    ssl: Any | None = None,
) -> Server:
    _require_io_wait_new()
    reader_ctor: Any = StreamReader
    writer_ctor: Any = StreamWriter
    if ssl is not None:
        use_tls = _require_ssl_transport_support("create_server", ssl, server_side=True)
        if use_tls:
            certfile, keyfile = _tls_server_payload(ssl)
            tls_handles: dict[int, Any] = {}

            def _tls_reader_ctor(conn: Any) -> ProcessStreamReader:
                if not isinstance(conn, _socket.socket):
                    raise TypeError(
                        "start_server ssl transport requires a stream socket connection"
                    )
                raw_fd = conn.detach()
                handle = _tls_server_from_fd(raw_fd, certfile, keyfile)
                tls_handles[id(conn)] = handle
                return ProcessStreamReader(handle)

            def _tls_writer_ctor(conn: Any) -> ProcessStreamWriter:
                if not isinstance(conn, _socket.socket):
                    raise TypeError(
                        "start_server ssl transport requires a stream socket connection"
                    )
                key = id(conn)
                handle = tls_handles.pop(key, None)
                if handle is None:
                    raw_fd = conn.detach()
                    handle = _tls_server_from_fd(raw_fd, certfile, keyfile)
                return ProcessStreamWriter(handle)

            reader_ctor = _tls_reader_ctor
            writer_ctor = _tls_writer_ctor
    bind_host = host if host is not None else "0.0.0.0"
    bind_port = 0 if port is None else port
    sock = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM)
    sock.setsockopt(_socket.SOL_SOCKET, _socket.SO_REUSEADDR, 1)
    if reuse_port and hasattr(_socket, "SO_REUSEPORT"):
        sock.setsockopt(_socket.SOL_SOCKET, int(getattr(_socket, "SO_REUSEPORT")), 1)
    sock.setblocking(False)
    sock.bind((bind_host, bind_port))
    sock.listen(backlog)
    return Server(
        sock, client_connected_cb, reader_ctor=reader_ctor, writer_ctor=writer_ctor
    )


async def start_unix_server(
    client_connected_cb: Any,
    path: str,
    *,
    backlog: int = 100,
    ssl: Any | None = None,
) -> Server:
    _require_unix_socket_support()
    _require_io_wait_new()
    reader_ctor: Any = StreamReader
    writer_ctor: Any = StreamWriter
    if ssl is not None:
        use_tls = _require_ssl_transport_support(
            "create_unix_server", ssl, server_side=True
        )
        if use_tls:
            certfile, keyfile = _tls_server_payload(ssl)
            tls_handles: dict[int, Any] = {}

            def _tls_reader_ctor(conn: Any) -> ProcessStreamReader:
                if not isinstance(conn, _socket.socket):
                    raise TypeError(
                        "start_unix_server ssl transport requires a stream socket connection"
                    )
                raw_fd = conn.detach()
                handle = _tls_server_from_fd(raw_fd, certfile, keyfile)
                tls_handles[id(conn)] = handle
                return ProcessStreamReader(handle)

            def _tls_writer_ctor(conn: Any) -> ProcessStreamWriter:
                if not isinstance(conn, _socket.socket):
                    raise TypeError(
                        "start_unix_server ssl transport requires a stream socket connection"
                    )
                key = id(conn)
                handle = tls_handles.pop(key, None)
                if handle is None:
                    raw_fd = conn.detach()
                    handle = _tls_server_from_fd(raw_fd, certfile, keyfile)
                return ProcessStreamWriter(handle)

            reader_ctor = _tls_reader_ctor
            writer_ctor = _tls_writer_ctor
    sock = _socket.socket(_socket.AF_UNIX, _socket.SOCK_STREAM)
    sock.setblocking(False)
    sock.bind(path)
    sock.listen(backlog)
    return Server(
        sock, client_connected_cb, reader_ctor=reader_ctor, writer_ctor=writer_ctor
    )


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
    try:
        return set(task_values)
    except Exception:
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
    tasks = [ensure_future(aw) for aw in aws]
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

    _asyncio_tasks_add_done_callback(tasks, _enqueue)

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
                    _asyncio_waiters_remove(self._putters, fut)
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
            _asyncio_waiters_notify(self._getters, 1, item)
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
                _asyncio_waiters_remove(self._getters, fut)
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
            _asyncio_waiters_notify(self._putters, 1, True)
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
            if self._getters:
                _asyncio_waiters_notify_exception(
                    self._getters, len(self._getters), _QueueShutDown()
                )
            if self._putters:
                _asyncio_waiters_notify_exception(
                    self._putters, len(self._putters), _QueueShutDown()
                )


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
    mod = _require_asyncio_intrinsic(_molt_module_new, "module_new")(name)
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
    logger = _logging.getLogger("asyncio")
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
        "PIPE": _SubprocessConstants.PIPE,
        "STDOUT": _SubprocessConstants.STDOUT,
        "DEVNULL": _SubprocessConstants.DEVNULL,
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
