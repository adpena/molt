"""Capability-gated asyncio shim for Molt."""

from __future__ import annotations
from typing import TYPE_CHECKING, Any, Callable, Iterable, Iterator, cast
from dataclasses import dataclass
import builtins as _builtins
from collections import deque as _deque
import heapq as _heapq
import logging as _logging
import os as _os
import sys as _sys
import time as _time
import traceback as _traceback
import contextlib as _contextlib
import inspect as _inspect
import errno as _errno
import socket as _socket
import io as _io
import types as _types
import threading as _threading
import reprlib as _reprlib
import linecache as _linecache
import typing
import signal as _signal
import functools as _functools
import collections as _collections
import selectors as _selectors
import concurrent as _concurrent
import itertools as _itertools
import stat as _stat
import warnings as _warnings
import weakref as _weakref
import ssl as _ssl
import subprocess as _subprocess

import contextvars as _contextvars
import enum as _enum

from _intrinsics import require_intrinsic as _intrinsic_require

_mod_dict = getattr(_sys.modules.get(__name__), "__dict__", None) or globals()

_VERSION_INFO = getattr(_sys, "version_info", (3, 12, 0, "final", 0))
_IS_WINDOWS = _os.name == "nt"
_EXPOSE_EVENT_LOOP = _VERSION_INFO >= (3, 13)
_EXPOSE_WINDOWS_POLICIES = _IS_WINDOWS
_EXPOSE_QUEUE_SHUTDOWN = _VERSION_INFO >= (3, 13)
_EXPOSE_GRAPH = _VERSION_INFO >= (3, 14)
_EXPOSE_CHILD_WATCHERS = _VERSION_INFO < (3, 14)

_BASE_ALL = [
    "AbstractEventLoop",
    "AbstractEventLoopPolicy",
    "AbstractServer",
    "BaseEventLoop",
    "BaseProtocol",
    "BaseTransport",
    "BufferedProtocol",
    "CancelledError",
    "Condition",
    "DatagramProtocol",
    "DatagramTransport",
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
    "Protocol",
    "FIRST_COMPLETED",
    "FIRST_EXCEPTION",
    "ALL_COMPLETED",
    "Queue",
    "QueueEmpty",
    "QueueFull",
    "BrokenBarrierError",
    "Barrier",
    "ReadTransport",
    "Runner",
    "SelectorEventLoop",
    "Semaphore",
    "BoundedSemaphore",
    "SendfileNotAvailableError",
    "Server",
    "StreamReader",
    "StreamWriter",
    "SubprocessProtocol",
    "SubprocessTransport",
    "Task",
    "TaskGroup",
    "TimerHandle",
    "TimeoutError",
    "Transport",
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
    "WriteTransport",
    "wrap_future",
    "wait",
    "wait_for",
]

__all__ = list(_BASE_ALL)
if _EXPOSE_CHILD_WATCHERS:
    __all__.extend(
        [
            "get_child_watcher",
            "set_child_watcher",
            "AbstractChildWatcher",
            "FastChildWatcher",
            "PidfdChildWatcher",
            "SafeChildWatcher",
            "ThreadedChildWatcher",
        ]
    )
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

    def molt_asyncio_event_loop_get_current() -> Any: ...

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

    def molt_event_loop_connect_read_pipe(
        _loop_handle: Any, _fd: Any, _callback: Any
    ) -> Any: ...

    def molt_event_loop_connect_write_pipe(
        _loop_handle: Any, _fd: Any, _callback: Any
    ) -> Any: ...

    def molt_pipe_transport_new(_fd: Any, _is_read: Any) -> Any: ...

    def molt_pipe_transport_get_fd(_handle: Any) -> int: ...

    def molt_pipe_transport_is_closing(_handle: Any) -> bool: ...

    def molt_pipe_transport_close(_handle: Any) -> None: ...

    def molt_pipe_transport_pause_reading(_handle: Any) -> None: ...

    def molt_pipe_transport_resume_reading(_handle: Any) -> None: ...

    def molt_pipe_transport_write(_handle: Any, _data: Any) -> None: ...

    def molt_pipe_transport_get_write_buffer_size(_handle: Any) -> int: ...

    def molt_pipe_transport_drop(_handle: Any) -> None: ...

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

    # Handle-based state machine intrinsics -- Future
    def molt_asyncio_future_new() -> int: ...

    def molt_asyncio_future_result(_handle: int) -> Any: ...

    def molt_asyncio_future_exception(_handle: int) -> Any: ...

    def molt_asyncio_future_set_result_fast(_handle: int, _result: Any) -> int: ...

    def molt_asyncio_future_set_exception_fast(_handle: int, _exc: Any) -> int: ...

    def molt_asyncio_future_cancel_fast(_handle: int, _msg: Any) -> bool: ...

    def molt_asyncio_future_done(_handle: int) -> bool: ...

    def molt_asyncio_future_cancelled(_handle: int) -> bool: ...

    def molt_asyncio_future_add_done_callback_fast(_handle: int, _cb: Any) -> bool: ...

    def molt_asyncio_future_drop(_handle: int) -> None: ...

    # Handle-based state machine intrinsics -- Event
    def molt_asyncio_event_new() -> int: ...

    def molt_asyncio_event_is_set(_handle: int) -> bool: ...

    def molt_asyncio_event_set_fast(_handle: int) -> int: ...

    def molt_asyncio_event_clear_handle(_handle: int) -> None: ...

    def molt_asyncio_event_drop(_handle: int) -> None: ...

    # Handle-based state machine intrinsics -- Lock
    def molt_asyncio_lock_new() -> int: ...

    def molt_asyncio_lock_locked(_handle: int) -> bool: ...

    def molt_asyncio_lock_acquire_fast(_handle: int) -> bool: ...

    def molt_asyncio_lock_release_fast(_handle: int) -> int: ...

    def molt_asyncio_lock_drop(_handle: int) -> None: ...

    # Handle-based state machine intrinsics -- Semaphore
    def molt_asyncio_semaphore_new(_value: int) -> int: ...

    def molt_asyncio_semaphore_acquire_fast(_handle: int) -> bool: ...

    def molt_asyncio_semaphore_release_fast(_handle: int, _max_value: int) -> int: ...

    def molt_asyncio_semaphore_value(_handle: int) -> int: ...

    def molt_asyncio_semaphore_drop(_handle: int) -> None: ...

    # Handle-based state machine intrinsics -- Queue
    def molt_asyncio_queue_new(_maxsize: int, _queue_type: int) -> int: ...

    def molt_asyncio_queue_put_nowait(_handle: int, _item: Any) -> None: ...

    def molt_asyncio_queue_get_nowait(_handle: int) -> Any: ...

    def molt_asyncio_queue_qsize(_handle: int) -> int: ...

    def molt_asyncio_queue_maxsize(_handle: int) -> int: ...

    def molt_asyncio_queue_empty(_handle: int) -> bool: ...

    def molt_asyncio_queue_full(_handle: int) -> bool: ...

    def molt_asyncio_queue_task_done(_handle: int) -> None: ...

    def molt_asyncio_queue_unfinished_tasks(_handle: int) -> int: ...

    def molt_asyncio_queue_putter_count(_handle: int) -> int: ...

    def molt_asyncio_queue_getter_count(_handle: int) -> int: ...

    def molt_asyncio_queue_add_putter(_handle: int, _waiter: Any) -> None: ...

    def molt_asyncio_queue_add_getter(_handle: int, _waiter: Any) -> None: ...

    def molt_asyncio_queue_notify_putters(_handle: int, _count: int) -> int: ...

    def molt_asyncio_queue_notify_getters(_handle: int, _count: int) -> int: ...

    def molt_asyncio_queue_shutdown(_handle: int, _immediate: bool) -> None: ...

    def molt_asyncio_queue_is_shutdown(_handle: int) -> bool: ...

    def molt_asyncio_queue_drop(_handle: int) -> None: ...

    # Handle-based event loop intrinsics (RT2 core)
    def molt_event_loop_new() -> Any: ...

    def molt_event_loop_call_soon(_loop_handle: Any, _callback: Any) -> None: ...

    def molt_event_loop_call_later(
        _loop_handle: Any, _delay: Any, _callback: Any
    ) -> Any: ...

    def molt_event_loop_call_at(
        _loop_handle: Any, _when: Any, _callback: Any
    ) -> Any: ...

    def molt_event_loop_cancel_timer(_loop_handle: Any, _timer_id: Any) -> None: ...

    def molt_event_loop_add_reader(
        _loop_handle: Any, _fd: Any, _callback: Any
    ) -> None: ...

    def molt_event_loop_remove_reader(_loop_handle: Any, _fd: Any) -> bool: ...

    def molt_event_loop_add_writer(
        _loop_handle: Any, _fd: Any, _callback: Any
    ) -> None: ...

    def molt_event_loop_remove_writer(_loop_handle: Any, _fd: Any) -> bool: ...

    def molt_event_loop_run_once(_loop_handle: Any) -> int: ...

    def molt_event_loop_time(_loop_handle: Any) -> float: ...

    def molt_event_loop_next_deadline_delay(_loop_handle: Any) -> float: ...

    def molt_event_loop_has_pending(_loop_handle: Any) -> bool: ...

    def molt_event_loop_ready_count(_loop_handle: Any) -> int: ...

    def molt_event_loop_start(_loop_handle: Any) -> None: ...

    def molt_event_loop_stop(_loop_handle: Any) -> None: ...

    def molt_event_loop_is_running(_loop_handle: Any) -> bool: ...

    def molt_event_loop_is_closed(_loop_handle: Any) -> bool: ...

    def molt_event_loop_close(_loop_handle: Any) -> None: ...

    def molt_event_loop_drop(_loop_handle: Any) -> None: ...

    def molt_event_loop_set_debug(_loop_handle: Any, _enabled: Any) -> None: ...

    def molt_event_loop_get_debug(_loop_handle: Any) -> bool: ...

    def molt_event_loop_set_exception_handler(
        _loop_handle: Any, _handler: Any
    ) -> None: ...

    def molt_event_loop_get_exception_handler(_loop_handle: Any) -> Any: ...

    def molt_event_loop_set_task_factory(_loop_handle: Any, _factory: Any) -> None: ...

    def molt_event_loop_get_task_factory(_loop_handle: Any) -> Any: ...

    def molt_event_loop_notify_reader_ready(_loop_handle: Any, _fd: Any) -> None: ...

    def molt_event_loop_notify_writer_ready(_loop_handle: Any, _fd: Any) -> None: ...


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
    @classmethod
    def __class_getitem__(cls, item: Any) -> Any:
        return _require_asyncio_intrinsic(molt_generic_alias_new, "generic_alias_new")(
            cls, item
        )

    def __init__(self) -> None:
        self._fut_handle: int = molt_asyncio_future_new()
        self._result: Any = None
        self._exception: BaseException | None = None
        self._cancel_message: Any | None = None
        self._molt_event_owner: Event | None = None
        self._molt_event_token_id: int | None = None
        if _EXPOSE_GRAPH:
            self._asyncio_awaited_by: set["Future"] | None = None
        self._callbacks: list[tuple[Callable[["Future"], Any], Any | None]] = []
        self._molt_promise: Any | None = molt_promise_new()
        self._loop: Any = molt_asyncio_running_loop_get()
        if _DEBUG_ASYNCIO_PROMISE:
            _debug_write(
                "asyncio_promise_new ok={ok} promise={promise}".format(
                    ok=self._molt_promise is not None,
                    promise=self._molt_promise,
                )
            )

    def cancel(self, msg: Any | None = None) -> bool:
        if molt_asyncio_future_done(self._fut_handle):
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
        molt_asyncio_future_cancel_fast(self._fut_handle, msg)
        self._exception = None
        self._cancel_message = None
        if msg is not None:
            if isinstance(msg, str) or isinstance(msg, bytes):
                self._exception = CancelledError(msg)
            else:
                self._exception = CancelledError()
            self._cancel_message = msg
        self._invoke_callbacks()
        return True

    def cancelled(self) -> bool:
        return bool(molt_asyncio_future_cancelled(self._fut_handle))

    def done(self) -> bool:
        return bool(molt_asyncio_future_done(self._fut_handle))

    def result(self) -> Any:
        if not molt_asyncio_future_done(self._fut_handle):
            raise InvalidStateError("Result is not ready")
        if molt_asyncio_future_cancelled(self._fut_handle):
            if self._exception is not None:
                raise self._exception
            raise CancelledError
        if self._exception is not None:
            if _DEBUG_ASYNCIO_EXC:
                exc_name = getattr(type(self._exception), "__name__", "Unknown")
                _debug_write("future_exception_type={name}".format(name=exc_name))
            _debug_exc_state("future_result_before_raise")
            raise self._exception
            _debug_exc_state("future_result_after_raise")
        return self._result

    def exception(self) -> BaseException | None:
        if not molt_asyncio_future_done(self._fut_handle):
            raise InvalidStateError("Result is not ready")
        if molt_asyncio_future_cancelled(self._fut_handle):
            if self._exception is not None:
                raise self._exception
            raise CancelledError
        return self._exception

    def add_done_callback(
        self, fn: Callable[["Future"], Any], *, context: Any | None = None
    ) -> None:
        if context is None:
            copy_ctx = getattr(_contextvars, "copy_context", None)
            if callable(copy_ctx):
                context = copy_ctx()
            else:
                context = None
        if molt_asyncio_future_done(self._fut_handle):
            self._run_callback(fn, context)
            return None
        self._callbacks.append((fn, context))
        return None

    def remove_done_callback(self, fn: Callable[["Future"], Any]) -> int:
        filtered = [(f, ctx) for f, ctx in self._callbacks if f is not fn]
        removed = len(self._callbacks) - len(filtered)
        self._callbacks[:] = filtered
        return removed

    def get_loop(self) -> Any:
        return self._loop

    def set_result(self, result: Any) -> None:
        if molt_asyncio_future_done(self._fut_handle):
            raise InvalidStateError("Result is already set")
        self._result = result
        molt_asyncio_future_set_result_fast(self._fut_handle, result)
        if self._molt_promise is not None:
            molt_promise_set_result(self._molt_promise, result)
        self._invoke_callbacks()

    def set_exception(self, exception: BaseException) -> None:
        if molt_asyncio_future_done(self._fut_handle):
            raise InvalidStateError("Result is already set")
        self._exception = exception
        if _is_cancelled_exc(exception):
            molt_asyncio_future_cancel_fast(self._fut_handle, None)
        else:
            molt_asyncio_future_set_exception_fast(self._fut_handle, exception)
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
        if context is not None:
            context.run(fn, self)
        else:
            fn(self)

    async def _wait(self) -> Any:
        while not molt_asyncio_future_done(self._fut_handle):
            await _async_yield_once()
        return self.result()

    def __await__(self) -> Any:
        promise = self._molt_promise
        if promise is None:
            raise RuntimeError("asyncio intrinsic not available: promise_new")

        async def _molt_future_await_promise() -> Any:
            waiter = None
            if _EXPOSE_GRAPH:
                waiter = _task_registry_current()
                if isinstance(waiter, Future):
                    future_add_to_awaited_by(self, waiter)
            try:
                if _DEBUG_ASYNCIO_PROMISE:
                    _debug_write("asyncio_promise_await")
                return await promise
            finally:
                if _EXPOSE_GRAPH and isinstance(waiter, Future):
                    future_discard_from_awaited_by(self, waiter)

        return _molt_future_await_promise().__await__()

    def __repr__(self) -> str:
        if molt_asyncio_future_cancelled(self._fut_handle):
            state = "cancelled"
        elif molt_asyncio_future_done(self._fut_handle):
            state = "finished"
        else:
            state = "pending"
        return f"<Future {state}>"

    def __del__(self) -> None:
        handle = getattr(self, "_fut_handle", None)
        if handle is not None:
            molt_asyncio_future_drop(handle)


def future_add_to_awaited_by(fut: Any, waiter: Any) -> None:
    if isinstance(fut, Future) and isinstance(waiter, Future):
        if fut._asyncio_awaited_by is None:
            fut._asyncio_awaited_by = set()
        fut._asyncio_awaited_by.add(waiter)


def future_discard_from_awaited_by(fut: Any, waiter: Any) -> None:
    if isinstance(fut, Future) and isinstance(waiter, Future):
        if fut._asyncio_awaited_by is not None:
            fut._asyncio_awaited_by.discard(waiter)


_DEBUG_GATHER = False
_DEBUG_WAIT_FOR = False
_DEBUG_TASKS = False
_DEBUG_ASYNCIO_PROMISE = False
_DEBUG_ASYNCIO_EXC = False
_DEBUG_ASYNCIO_CONDITION = False

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


def _socket_eai_codes() -> set[int]:
    codes: set[int] = set()
    for name in dir(_socket):
        if not name.startswith("EAI_"):
            continue
        value = getattr(_socket, name, None)
        if isinstance(value, int):
            codes.add(value)
    return codes


_SOCKET_EAI_CODES = _socket_eai_codes()


def _map_socket_name_resolution_error(exc: OSError) -> OSError:
    errno_value = getattr(exc, "errno", None)
    if isinstance(errno_value, int) and errno_value in _SOCKET_EAI_CODES:
        gaierror_cls = getattr(_socket, "gaierror", None)
        if gaierror_cls is not None:
            return gaierror_cls(errno_value, str(exc))
    return exc


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
molt_spawn = _intrinsic_require("molt_spawn", globals())
molt_cancel_token_new = _intrinsic_require("molt_cancel_token_new", globals())
molt_cancel_token_clone = _intrinsic_require("molt_cancel_token_clone", globals())
molt_cancel_token_drop = _intrinsic_require("molt_cancel_token_drop", globals())
molt_cancel_token_cancel = _intrinsic_require("molt_cancel_token_cancel", globals())
molt_cancel_token_is_cancelled = _intrinsic_require(
    "molt_cancel_token_is_cancelled", globals()
)
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
molt_asyncio_event_loop_get_current = _intrinsic_require(
    "molt_asyncio_event_loop_get_current", globals()
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
molt_event_loop_connect_read_pipe = _intrinsic_require(
    "molt_event_loop_connect_read_pipe", globals()
)
molt_event_loop_connect_write_pipe = _intrinsic_require(
    "molt_event_loop_connect_write_pipe", globals()
)
# --- Event loop Rust handle intrinsics (RT2 core, 28 total) ---
molt_event_loop_new = _intrinsic_require("molt_event_loop_new", globals())
molt_event_loop_call_soon = _intrinsic_require("molt_event_loop_call_soon", globals())
molt_event_loop_call_later = _intrinsic_require("molt_event_loop_call_later", globals())
molt_event_loop_call_at = _intrinsic_require("molt_event_loop_call_at", globals())
molt_event_loop_cancel_timer = _intrinsic_require(
    "molt_event_loop_cancel_timer", globals()
)
molt_event_loop_add_reader = _intrinsic_require("molt_event_loop_add_reader", globals())
molt_event_loop_remove_reader = _intrinsic_require(
    "molt_event_loop_remove_reader", globals()
)
molt_event_loop_add_writer = _intrinsic_require("molt_event_loop_add_writer", globals())
molt_event_loop_remove_writer = _intrinsic_require(
    "molt_event_loop_remove_writer", globals()
)
molt_event_loop_run_once = _intrinsic_require("molt_event_loop_run_once", globals())
molt_event_loop_time = _intrinsic_require("molt_event_loop_time", globals())
molt_event_loop_next_deadline_delay = _intrinsic_require(
    "molt_event_loop_next_deadline_delay", globals()
)
molt_event_loop_has_pending = _intrinsic_require(
    "molt_event_loop_has_pending", globals()
)
molt_event_loop_ready_count = _intrinsic_require(
    "molt_event_loop_ready_count", globals()
)
molt_event_loop_start = _intrinsic_require("molt_event_loop_start", globals())
molt_event_loop_stop = _intrinsic_require("molt_event_loop_stop", globals())
molt_event_loop_is_running = _intrinsic_require("molt_event_loop_is_running", globals())
molt_event_loop_is_closed = _intrinsic_require("molt_event_loop_is_closed", globals())
molt_event_loop_close = _intrinsic_require("molt_event_loop_close", globals())
molt_event_loop_drop = _intrinsic_require("molt_event_loop_drop", globals())
molt_event_loop_set_debug = _intrinsic_require("molt_event_loop_set_debug", globals())
molt_event_loop_get_debug = _intrinsic_require("molt_event_loop_get_debug", globals())
molt_event_loop_set_exception_handler = _intrinsic_require(
    "molt_event_loop_set_exception_handler", globals()
)
molt_event_loop_get_exception_handler = _intrinsic_require(
    "molt_event_loop_get_exception_handler", globals()
)
molt_event_loop_set_task_factory = _intrinsic_require(
    "molt_event_loop_set_task_factory", globals()
)
molt_event_loop_get_task_factory = _intrinsic_require(
    "molt_event_loop_get_task_factory", globals()
)
molt_event_loop_notify_reader_ready = _intrinsic_require(
    "molt_event_loop_notify_reader_ready", globals()
)
molt_event_loop_notify_writer_ready = _intrinsic_require(
    "molt_event_loop_notify_writer_ready", globals()
)
molt_pipe_transport_new = _intrinsic_require("molt_pipe_transport_new", globals())
molt_pipe_transport_get_fd = _intrinsic_require("molt_pipe_transport_get_fd", globals())
molt_pipe_transport_is_closing = _intrinsic_require(
    "molt_pipe_transport_is_closing", globals()
)
molt_pipe_transport_close = _intrinsic_require("molt_pipe_transport_close", globals())
molt_pipe_transport_pause_reading = _intrinsic_require(
    "molt_pipe_transport_pause_reading", globals()
)
molt_pipe_transport_resume_reading = _intrinsic_require(
    "molt_pipe_transport_resume_reading", globals()
)
molt_pipe_transport_write = _intrinsic_require("molt_pipe_transport_write", globals())
molt_pipe_transport_get_write_buffer_size = _intrinsic_require(
    "molt_pipe_transport_get_write_buffer_size", globals()
)
molt_pipe_transport_drop = _intrinsic_require("molt_pipe_transport_drop", globals())
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
molt_generic_alias_new = _intrinsic_require("molt_generic_alias_new", globals())
molt_thread_submit = _intrinsic_require("molt_thread_submit", globals())
molt_asyncio_to_thread = _intrinsic_require("molt_asyncio_to_thread", globals())
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

# Handle-based state machine intrinsics -- Future
molt_asyncio_future_new = _intrinsic_require("molt_asyncio_future_new", globals())
molt_asyncio_future_result = _intrinsic_require("molt_asyncio_future_result", globals())
molt_asyncio_future_exception = _intrinsic_require(
    "molt_asyncio_future_exception", globals()
)
molt_asyncio_future_set_result_fast = _intrinsic_require(
    "molt_asyncio_future_set_result_fast", globals()
)
molt_asyncio_future_set_exception_fast = _intrinsic_require(
    "molt_asyncio_future_set_exception_fast", globals()
)
molt_asyncio_future_cancel_fast = _intrinsic_require(
    "molt_asyncio_future_cancel_fast", globals()
)
molt_asyncio_future_done = _intrinsic_require("molt_asyncio_future_done", globals())
molt_asyncio_future_cancelled = _intrinsic_require(
    "molt_asyncio_future_cancelled", globals()
)
molt_asyncio_future_add_done_callback_fast = _intrinsic_require(
    "molt_asyncio_future_add_done_callback_fast", globals()
)
molt_asyncio_future_drop = _intrinsic_require("molt_asyncio_future_drop", globals())

# Handle-based state machine intrinsics -- Event
molt_asyncio_event_new = _intrinsic_require("molt_asyncio_event_new", globals())
molt_asyncio_event_is_set = _intrinsic_require("molt_asyncio_event_is_set", globals())
molt_asyncio_event_set_fast = _intrinsic_require(
    "molt_asyncio_event_set_fast", globals()
)
molt_asyncio_event_clear_handle = _intrinsic_require(
    "molt_asyncio_event_clear", globals()
)
molt_asyncio_event_drop = _intrinsic_require("molt_asyncio_event_drop", globals())

# Handle-based state machine intrinsics -- Lock
molt_asyncio_lock_new = _intrinsic_require("molt_asyncio_lock_new", globals())
molt_asyncio_lock_locked = _intrinsic_require("molt_asyncio_lock_locked", globals())
molt_asyncio_lock_acquire_fast = _intrinsic_require(
    "molt_asyncio_lock_acquire_fast", globals()
)
molt_asyncio_lock_release_fast = _intrinsic_require(
    "molt_asyncio_lock_release_fast", globals()
)
molt_asyncio_lock_drop = _intrinsic_require("molt_asyncio_lock_drop", globals())

# Handle-based state machine intrinsics -- Semaphore
molt_asyncio_semaphore_new = _intrinsic_require("molt_asyncio_semaphore_new", globals())
molt_asyncio_semaphore_acquire_fast = _intrinsic_require(
    "molt_asyncio_semaphore_acquire_fast", globals()
)
molt_asyncio_semaphore_release_fast = _intrinsic_require(
    "molt_asyncio_semaphore_release_fast", globals()
)
molt_asyncio_semaphore_value = _intrinsic_require(
    "molt_asyncio_semaphore_value", globals()
)
molt_asyncio_semaphore_drop = _intrinsic_require(
    "molt_asyncio_semaphore_drop", globals()
)

# Handle-based state machine intrinsics -- Queue
molt_asyncio_queue_new = _intrinsic_require("molt_asyncio_queue_new", globals())
molt_asyncio_queue_put_nowait = _intrinsic_require(
    "molt_asyncio_queue_put_nowait", globals()
)
molt_asyncio_queue_get_nowait = _intrinsic_require(
    "molt_asyncio_queue_get_nowait", globals()
)
molt_asyncio_queue_qsize = _intrinsic_require("molt_asyncio_queue_qsize", globals())
molt_asyncio_queue_maxsize = _intrinsic_require("molt_asyncio_queue_maxsize", globals())
molt_asyncio_queue_empty = _intrinsic_require("molt_asyncio_queue_empty", globals())
molt_asyncio_queue_full = _intrinsic_require("molt_asyncio_queue_full", globals())
molt_asyncio_queue_task_done = _intrinsic_require(
    "molt_asyncio_queue_task_done", globals()
)
molt_asyncio_queue_unfinished_tasks = _intrinsic_require(
    "molt_asyncio_queue_unfinished_tasks", globals()
)
molt_asyncio_queue_shutdown = _intrinsic_require(
    "molt_asyncio_queue_shutdown", globals()
)
molt_asyncio_queue_is_shutdown = _intrinsic_require(
    "molt_asyncio_queue_is_shutdown", globals()
)
molt_asyncio_queue_drop = _intrinsic_require("molt_asyncio_queue_drop", globals())
molt_asyncio_queue_putter_count = _intrinsic_require(
    "molt_asyncio_queue_putter_count", globals()
)
molt_asyncio_queue_getter_count = _intrinsic_require(
    "molt_asyncio_queue_getter_count", globals()
)
molt_asyncio_queue_add_putter = _intrinsic_require(
    "molt_asyncio_queue_add_putter", globals()
)
molt_asyncio_queue_add_getter = _intrinsic_require(
    "molt_asyncio_queue_add_getter", globals()
)
molt_asyncio_queue_notify_putters = _intrinsic_require(
    "molt_asyncio_queue_notify_putters", globals()
)
molt_asyncio_queue_notify_getters = _intrinsic_require(
    "molt_asyncio_queue_notify_getters", globals()
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
    pending = _molt_exception_pending() if _molt_exception_pending is not None else 0
    last_obj = (
        _molt_exception_last() if pending and _molt_exception_last is not None else None
    )
    last_type = (
        getattr(type(last_obj), "__name__", "None") if last_obj is not None else "None"
    )
    _debug_write(
        "asyncio_exc tag={tag} pending={pending} last={last}".format(
            tag=tag, pending=int(bool(pending)), last=last_type
        )
    )
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


def _socket_wait_key(sock: Any) -> Any:
    fileno = getattr(sock, "fileno", None)
    if callable(fileno):
        return int(fileno())
    handle_getter = getattr(sock, "_require_handle", None)
    if callable(handle_getter):
        return handle_getter()
    raise TypeError("socket object must provide fileno() or _require_handle()")


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
        if not hasattr(self._sock, "settimeout"):
            return None
        prev = self._sock.gettimeout()
        self._prev = prev
        self._sock.settimeout(0.0)
        return None

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        if self._prev is _UNSET:
            return None
        if hasattr(self._sock, "settimeout"):
            self._sock.settimeout(self._prev)
        return None


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


def _debug_write(message: str) -> None:
    err = getattr(_sys, "stderr", None)
    if err is None or not hasattr(err, "write"):
        err = getattr(_sys, "__stderr__", None)
    if err is not None and hasattr(err, "write"):
        err.write(f"{message}\n")
        flush_fn = getattr(err, "flush", None)
        if callable(flush_fn):
            flush_fn()
        return None
    out = getattr(_sys, "stdout", None)
    if out is not None and hasattr(out, "write"):
        out.write(f"{message}\n")
        flush_fn = getattr(out, "flush", None)
        if callable(flush_fn):
            flush_fn()
        return None
    print(message)


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
            if not molt_asyncio_future_done(self._fut_handle):
                self.set_result(result)
                if _DEBUG_TASKS:
                    token_id = self._token.token_id()
                    _debug_write(f"asyncio_task_done token={token_id}")
        else:
            if not molt_asyncio_future_done(self._fut_handle):
                self.set_exception(exc)
        _cleanup_event_waiters_for_token(self._token.token_id())
        _task_registry_pop(self._token.token_id())
        if extra_token_id is not None:
            _task_registry_pop(extra_token_id)
        _contextvars._clear_context_for_token(  # type: ignore[unresolved-attribute]
            self._token.token_id()
        )

    def __repr__(self) -> str:
        if molt_asyncio_future_cancelled(self._fut_handle):
            state = "cancelled"
        elif molt_asyncio_future_done(self._fut_handle):
            state = "finished"
        else:
            state = "pending"
        return f"<Task {self._name} {state}>"

    def __await__(self) -> Any:
        return Future.__await__(self)


class Event:
    def __init__(self) -> None:
        self._evt_handle: int = molt_asyncio_event_new()
        self._waiters: list[Future] = []

    def is_set(self) -> bool:
        return bool(molt_asyncio_event_is_set(self._evt_handle))

    def set(self) -> None:
        if molt_asyncio_event_is_set(self._evt_handle):
            return None
        molt_asyncio_event_set_fast(self._evt_handle)
        waiters = self._waiters
        self._waiters = []
        _require_asyncio_intrinsic(
            molt_asyncio_event_set_waiters, "asyncio_event_set_waiters"
        )(waiters, True)
        return None

    def clear(self) -> None:
        molt_asyncio_event_clear_handle(self._evt_handle)

    async def wait(self) -> bool:
        if molt_asyncio_event_is_set(self._evt_handle):
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

    def __repr__(self) -> str:
        state = "set" if self.is_set() else "unset"
        return f"<Event [{state}]>"

    def __del__(self) -> None:
        handle = getattr(self, "_evt_handle", None)
        if handle is not None:
            molt_asyncio_event_drop(handle)


class Lock:
    def __init__(self) -> None:
        self._lock_handle: int = molt_asyncio_lock_new()
        self._waiters: _deque[Future] = _deque()

    def locked(self) -> bool:
        return bool(molt_asyncio_lock_locked(self._lock_handle))

    async def acquire(self) -> bool:
        if molt_asyncio_lock_acquire_fast(self._lock_handle):
            return True
        fut = Future()
        self._waiters.append(fut)
        try:
            await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                _asyncio_waiters_remove(self._waiters, fut)
            raise
        molt_asyncio_lock_acquire_fast(self._lock_handle)
        return True

    def release(self) -> None:
        if not molt_asyncio_lock_locked(self._lock_handle):
            raise RuntimeError("Lock is not acquired")
        molt_asyncio_lock_release_fast(self._lock_handle)
        if self._waiters:
            _asyncio_waiters_notify(self._waiters, 1, True)

    async def __aenter__(self) -> "Lock":
        await self.acquire()
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.release()

    def __repr__(self) -> str:
        state = "locked" if self.locked() else "unlocked"
        return f"<Lock [{state}]>"

    def __del__(self) -> None:
        handle = getattr(self, "_lock_handle", None)
        if handle is not None:
            molt_asyncio_lock_drop(handle)


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

    def __repr__(self) -> str:
        state = "locked" if self.locked() else "unlocked"
        waiters = len(self._waiters)
        return f"<Condition [{state}, waiters:{waiters}]>"


class Semaphore:
    def __init__(self, value: int = 1) -> None:
        if value < 0:
            raise ValueError("Semaphore initial value must be >= 0")
        self._sem_handle: int = molt_asyncio_semaphore_new(value)
        self._waiters: _deque[Future] = _deque()

    def locked(self) -> bool:
        return molt_asyncio_semaphore_value(self._sem_handle) == 0

    async def acquire(self) -> bool:
        if molt_asyncio_semaphore_acquire_fast(self._sem_handle):
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
        molt_asyncio_semaphore_release_fast(self._sem_handle, -1)
        if self._waiters:
            _asyncio_waiters_notify(self._waiters, 1, True)

    async def __aenter__(self) -> "Semaphore":
        await self.acquire()
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.release()

    def __repr__(self) -> str:
        value = int(molt_asyncio_semaphore_value(self._sem_handle))
        state = "locked" if value == 0 else f"unlocked, value:{value}"
        return f"<Semaphore [{state}]>"

    def __del__(self) -> None:
        handle = getattr(self, "_sem_handle", None)
        if handle is not None:
            molt_asyncio_semaphore_drop(handle)


class BoundedSemaphore(Semaphore):
    def __init__(self, value: int = 1) -> None:
        super().__init__(value)
        self._initial_value = value

    def release(self) -> None:
        molt_asyncio_semaphore_release_fast(self._sem_handle, self._initial_value)
        if self._waiters:
            _asyncio_waiters_notify(self._waiters, 1, True)


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
            # Set the result of each waiter to its index.
            waiters = self._waiters
            self._waiters = []
            for i, w in enumerate(waiters):
                if not w.done():
                    w.set_result(i)
        try:
            return await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                _asyncio_waiters_remove(self._waiters, fut)
                if self._count > 0:
                    self._count -= 1
            raise

    @property
    def parties(self) -> int:
        return self._parties

    @property
    def n_waiting(self) -> int:
        return self._count

    @property
    def broken(self) -> bool:
        return self._broken

    async def reset(self) -> None:
        waiters = self._waiters
        self._waiters = []
        self._count = 0
        self._broken = False
        # Wake all waiters with BrokenBarrierError.
        for w in waiters:
            if not w.done():
                w.set_exception(BrokenBarrierError("Barrier was reset"))

    async def abort(self) -> None:
        self._broken = True
        waiters = self._waiters
        self._waiters = []
        self._count = 0
        # Wake all waiters with BrokenBarrierError.
        for w in waiters:
            if not w.done():
                w.set_exception(BrokenBarrierError("Barrier was aborted"))

    async def __aenter__(self) -> int:
        return await self.wait()

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        pass

    def __repr__(self) -> str:
        if self._broken:
            state = "broken"
        else:
            state = "filling"
        return f"<Barrier [{state}, waiters:{self._count}/{self._parties}]>"


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
        if hasattr(self._sock, "setblocking"):
            self._sock.setblocking(False)
        fileno_fn = getattr(self._sock, "fileno", None)
        if callable(fileno_fn):
            self._fd = fileno_fn()
        else:
            self._fd = -1
        self._wait_key = _socket_wait_key(self._sock)
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
                )(self._reader, n, self._wait_key)
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

    def feed_eof(self) -> None:
        self._eof = True

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
                )(self._reader, self._wait_key)
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
        if _molt_socket_reader_drop is not None:
            _molt_socket_reader_drop(reader)


class StreamWriter:
    def __init__(self, sock: _socket.socket) -> None:
        self._sock = sock
        self._buffer = bytearray()
        self._closed = False
        self._transport: Any = None
        if hasattr(self._sock, "setblocking"):
            self._sock.setblocking(False)
        fileno_fn = getattr(self._sock, "fileno", None)
        if callable(fileno_fn):
            self._fd = fileno_fn()
        else:
            self._fd = -1

    def write(self, data: bytes) -> None:
        if self._closed:
            return None
        if not isinstance(data, (bytes, bytearray, memoryview)):
            raise TypeError("data must be bytes-like")
        self._buffer.extend(data)
        return None

    def writelines(self, data: Iterable[bytes | bytearray | memoryview]) -> None:
        for chunk in data:
            self.write(chunk)

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
        if not self._closed and hasattr(self._sock, "shutdown"):
            self._sock.shutdown(_socket.SHUT_WR)

    def can_write_eof(self) -> bool:
        return True

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        if hasattr(self._sock, "close"):
            self._sock.close()

    def is_closing(self) -> bool:
        return self._closed

    async def wait_closed(self) -> None:
        return None

    def get_extra_info(self, name: str, default: Any = None) -> Any:
        transport = self._transport
        if transport is not None:
            get_info = getattr(transport, "get_extra_info", None)
            if callable(get_info):
                return get_info(name, default)
        return default

    async def start_tls(
        self,
        sslcontext: Any,
        *,
        server_hostname: str | None = None,
        ssl_handshake_timeout: float | None = None,
        ssl_shutdown_timeout: float | None = None,
    ) -> None:
        """Upgrade this stream connection to TLS."""
        loop = get_running_loop()
        transport = self._transport
        if transport is None:
            # Build a lightweight transport shim so _EventLoop.start_tls
            # can extract _sock and mark _closed after detach.
            transport = _StreamWriterTransportShim(self._sock)
            self._transport = transport
        protocol = Protocol()
        new_transport = await loop.start_tls(
            transport,
            protocol,
            sslcontext,
            server_hostname=server_hostname,
            ssl_handshake_timeout=ssl_handshake_timeout,
            ssl_shutdown_timeout=ssl_shutdown_timeout,
        )
        if new_transport is not None:
            self._transport = new_transport

    @property
    def transport(self) -> Any:
        return self._transport


class _StreamWriterTransportShim:
    """Minimal transport facade used by StreamWriter.start_tls.

    Exposes ``_sock`` and ``_closed`` so that ``_EventLoop.start_tls`` can
    call ``sock.detach()`` and mark the shim as closed without requiring a
    full ``_WritePipeTransport`` instantiation.
    """

    def __init__(self, sock: Any) -> None:
        self._sock = sock
        self._closed = False


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
        self._serving = True
        self._loop = get_running_loop()
        self._accept_task = self._loop.create_task(
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
        self._serving = False
        if hasattr(self._sock, "close"):
            self._sock.close()
        if self._accept_task is not None and not self._accept_task.done():
            self._accept_task.cancel()

    async def wait_closed(self) -> None:
        task = self._accept_task
        if not task.done():
            task.cancel()
            # Yield once so cancellation can propagate, but never block forever.
            await sleep(0.0)
            if not task.done():
                return None
        try:
            await task
        except BaseException:
            return None

    def is_serving(self) -> bool:
        return self._serving and not self._closed

    async def start_serving(self) -> None:
        self._serving = True

    async def serve_forever(self) -> None:
        self._serving = True
        if self._accept_task is not None:
            await self._accept_task

    def get_loop(self) -> "EventLoop":
        return self._loop

    def close_clients(self) -> None:
        # Molt tracks connections at the Rust level; closing the server
        # socket is sufficient to stop new connections.  Existing
        # connections drain naturally.
        pass

    def abort_clients(self) -> None:
        # Same as close_clients -- individual connection tracking is not
        # exposed at the Python level.
        pass


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
        if _molt_stream_reader_drop is not None:
            _molt_stream_reader_drop(reader)


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
        if _molt_stream_close is not None:
            _molt_stream_close(self._handle)

    async def wait_closed(self) -> None:
        return None


_PROCESS_WAIT_FUTURES: dict[int, Any] = {}


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
        key = id(self)
        wait_future = _PROCESS_WAIT_FUTURES.get(key)
        if wait_future is None:
            fut = _require_asyncio_intrinsic(
                _molt_process_wait_future, "process_wait_future"
            )(self._handle)
            _PROCESS_WAIT_FUTURES[key] = fut
            wait_future = fut
        code = int(await wait_future)
        watcher = _CHILD_WATCHER
        if watcher is not None and hasattr(watcher, "_notify_child_exit"):
            watcher._notify_child_exit(self.pid, code)
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

        tasks: list[Future] = []
        task_kinds: list[str] = []
        if self.stdout is not None:
            tasks.append(ensure_future(self.stdout.read()))
            task_kinds.append("stdout")
        if self.stderr is not None:
            tasks.append(ensure_future(self.stderr.read()))
            task_kinds.append("stderr")

        out: bytes | None = None
        err: bytes | None = None
        try:
            if tasks:
                while True:
                    current = current_task()
                    if current is not None and current.cancelling() > 0:
                        raise CancelledError()
                    all_done = True
                    for task in tasks:
                        if not task.done():
                            all_done = False
                            break
                    if all_done:
                        break
                    await sleep(0.0)
                results: list[Any] = []
                for task in tasks:
                    results.append(await task)
                for idx, result in enumerate(results):
                    kind = task_kinds[idx]
                    if kind == "stdout":
                        out = result
                    else:
                        err = result
            current = current_task()
            if current is not None and current.cancelling() > 0:
                raise CancelledError()
            await self.wait()
        except BaseException:
            for task in tasks:
                if not task.done():
                    task.cancel()
            raise
        return out, err

    def __del__(self) -> None:
        _PROCESS_WAIT_FUTURES.pop(id(self), None)
        if _molt_process_drop is not None:
            _molt_process_drop(self._handle)


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
        if self._context is not None:
            self._context.run(self._callback, *self._args)
        else:
            self._callback(*self._args)


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
        # Allocate a Rust-owned event loop handle.  All state for timing,
        # I/O readiness, timers, debug mode, exception handler, and task
        # factory is stored inside this handle; Python attributes below are
        # kept only for compatibility with code that reads them directly.
        self._loop_handle: Any = _require_asyncio_intrinsic(
            molt_event_loop_new, "event_loop_new"
        )()
        self._readers: dict[int, tuple[Any, tuple[Any, ...], Task]] = {}
        self._writers: dict[int, tuple[Any, tuple[Any, ...], Task]] = {}
        self._ready: _deque[Handle] = _deque()
        self._ready_lock = _threading.Lock()
        self._scheduled: set[TimerHandle] = set()
        self._ready_task: Task | None = None
        self._stopping = False
        self._default_executor: Any | None = None
        self._selector = selector
        self._signal_handlers: dict[
            int, tuple[Callable[..., Any], tuple[Any, ...]]
        ] = {}

    def __del__(self) -> None:
        handle = getattr(self, "_loop_handle", None)
        if handle is not None and molt_event_loop_drop is not None:  # type: ignore[name-defined]
            try:
                molt_event_loop_drop(handle)  # type: ignore[name-defined]
            except Exception:
                pass

    def create_future(self) -> Future:
        return Future()

    def create_task(
        self, coro: Any, *, name: str | None = None, context: Any | None = None
    ) -> Task:
        factory = self.get_task_factory()
        if factory is None:
            return Task(coro, loop=self, name=name, context=context)
        if context is None:
            task = factory(self, coro)
        else:
            task = factory(self, coro, context=context)
        if name is not None:
            setter = getattr(task, "set_name", None)
            if callable(setter):
                setter(name)
            else:
                setattr(task, "_name", name)
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
        if self.is_closed():
            raise RuntimeError("Event loop is closed")
        if context is None:
            copy_ctx = getattr(_contextvars, "copy_context", None)
            if callable(copy_ctx):
                context = copy_ctx()
            else:
                context = None
        handle = Handle(callback, args, self, context)
        # Notify Rust handle-level event loop of the immediate callback.
        _require_asyncio_intrinsic(molt_event_loop_call_soon, "event_loop_call_soon")(
            self._loop_handle, handle
        )
        # Enqueue in the Python-visible ready deque used by the coroutine runner.
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
        if self.is_closed():
            raise RuntimeError("Event loop is closed")
        if context is None:
            copy_ctx = getattr(_contextvars, "copy_context", None)
            if callable(copy_ctx):
                context = copy_ctx()
            else:
                context = None
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
        # Register with Rust event loop for timer tracking; returns an opaque
        # timer_id that the handle can use for cancellation.
        timer_id = _require_asyncio_intrinsic(
            molt_event_loop_call_later, "event_loop_call_later"
        )(self._loop_handle, float(delay), handle)
        if timer_id is not None:
            handle._rust_timer_id = timer_id
        # Also schedule through the existing asyncio timer infrastructure.
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
        if self.is_closed():
            raise RuntimeError("Event loop is closed")
        if context is None:
            copy_ctx = getattr(_contextvars, "copy_context", None)
            if callable(copy_ctx):
                context = copy_ctx()
            else:
                context = None
        delay = max(0.0, float(when) - self.time())
        handle = TimerHandle(float(when), callback, args, self, context)
        # Register with Rust event loop for timer tracking.
        timer_id = _require_asyncio_intrinsic(
            molt_event_loop_call_at, "event_loop_call_at"
        )(self._loop_handle, float(when), handle)
        if timer_id is not None:
            handle._rust_timer_id = timer_id
        # Also schedule through the existing asyncio timer infrastructure.
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
        _require_asyncio_intrinsic(
            molt_event_loop_set_exception_handler, "event_loop_set_exception_handler"
        )(self._loop_handle, handler)

    def get_exception_handler(
        self,
    ) -> Callable[["EventLoop", dict[str, Any]], Any] | None:
        return _require_asyncio_intrinsic(
            molt_event_loop_get_exception_handler, "event_loop_get_exception_handler"
        )(self._loop_handle)

    def call_exception_handler(self, context: dict[str, Any]) -> None:
        handler = self.get_exception_handler()
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
        _require_asyncio_intrinsic(molt_event_loop_set_debug, "event_loop_set_debug")(
            self._loop_handle, bool(enabled)
        )

    def get_debug(self) -> bool:
        return bool(
            _require_asyncio_intrinsic(
                molt_event_loop_get_debug, "event_loop_get_debug"
            )(self._loop_handle)
        )

    def set_task_factory(self, factory: Callable[..., Task] | None) -> None:
        _require_asyncio_intrinsic(
            molt_event_loop_set_task_factory, "event_loop_set_task_factory"
        )(self._loop_handle, factory)

    def get_task_factory(self) -> Callable[..., Task] | None:
        return _require_asyncio_intrinsic(
            molt_event_loop_get_task_factory, "event_loop_get_task_factory"
        )(self._loop_handle)

    def time(self) -> float:
        return float(
            _require_asyncio_intrinsic(molt_event_loop_time, "event_loop_time")(
                self._loop_handle
            )
        )

    def is_running(self) -> bool:
        return bool(
            _require_asyncio_intrinsic(
                molt_event_loop_is_running, "event_loop_is_running"
            )(self._loop_handle)
        )

    def is_closed(self) -> bool:
        return bool(
            _require_asyncio_intrinsic(
                molt_event_loop_is_closed, "event_loop_is_closed"
            )(self._loop_handle)
        )

    def stop(self) -> None:
        self._stopping = True
        _require_asyncio_intrinsic(molt_event_loop_stop, "event_loop_stop")(
            self._loop_handle
        )

    def close(self) -> None:
        if self.is_closed():
            return
        _require_asyncio_intrinsic(molt_event_loop_close, "event_loop_close")(
            self._loop_handle
        )
        if self._ready_task is not None and not self._ready_task.done():
            self._ready_task.cancel()
        if self._selector is not None and hasattr(self._selector, "close"):
            self._selector.close()

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
        # Register with Rust event loop for I/O readiness notification.
        _require_asyncio_intrinsic(molt_event_loop_add_reader, "event_loop_add_reader")(
            self._loop_handle, fileno, callback
        )
        _require_asyncio_intrinsic(
            molt_asyncio_fd_watcher_register, "asyncio_fd_watcher_register"
        )(self, self._readers, fileno, callback, args, 1)

    def remove_reader(self, fd: Any) -> bool:
        fileno = _fd_from_fileobj(fd)
        _require_asyncio_intrinsic(
            molt_event_loop_remove_reader, "event_loop_remove_reader"
        )(self._loop_handle, fileno)
        return bool(
            _require_asyncio_intrinsic(
                molt_asyncio_fd_watcher_unregister, "asyncio_fd_watcher_unregister"
            )(self._readers, fileno)
        )

    def add_writer(self, fd: Any, callback: Any, *args: Any) -> None:
        fileno = _fd_from_fileobj(fd)
        # Register with Rust event loop for I/O writability notification.
        _require_asyncio_intrinsic(molt_event_loop_add_writer, "event_loop_add_writer")(
            self._loop_handle, fileno, callback
        )
        _require_asyncio_intrinsic(
            molt_asyncio_fd_watcher_register, "asyncio_fd_watcher_register"
        )(self, self._writers, fileno, callback, args, 2)

    async def sock_recv(self, sock: Any, n: int) -> bytes:
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_recv_new, "asyncio_sock_recv_new"
        )(sock, n, _socket_wait_key(sock))
        return await fut

    async def sock_recv_into(self, sock: Any, buf: Any) -> int:
        nbytes = len(buf)
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_recv_into_new, "asyncio_sock_recv_into_new"
        )(sock, buf, nbytes, _socket_wait_key(sock))
        return await fut

    async def sock_sendall(self, sock: Any, data: bytes) -> None:
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_sendall_new, "asyncio_sock_sendall_new"
        )(sock, data, _socket_wait_key(sock))
        await fut

    async def sock_recvfrom(self, sock: Any, bufsize: int) -> tuple[Any, Any]:
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_recvfrom_new, "asyncio_sock_recvfrom_new"
        )(sock, bufsize, _socket_wait_key(sock))
        return await fut

    async def sock_recvfrom_into(self, sock: Any, buf: Any) -> tuple[int, Any]:
        nbytes = len(buf)
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_recvfrom_into_new, "asyncio_sock_recvfrom_into_new"
        )(sock, buf, nbytes, _socket_wait_key(sock))
        return await fut

    async def sock_sendto(self, sock: Any, data: bytes, addr: Any) -> int:
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_sendto_new, "asyncio_sock_sendto_new"
        )(sock, data, addr, _socket_wait_key(sock))
        return await fut

    async def sock_connect(self, sock: Any, address: Any) -> None:
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_connect_new, "asyncio_sock_connect_new"
        )(sock, address, _socket_wait_key(sock))
        await fut

    async def sock_accept(self, sock: Any) -> tuple[Any, Any]:
        fut = _require_asyncio_intrinsic(
            molt_asyncio_sock_accept_new, "asyncio_sock_accept_new"
        )(sock, _socket_wait_key(sock))
        return await fut

    def remove_writer(self, fd: Any) -> bool:
        fileno = _fd_from_fileobj(fd)
        _require_asyncio_intrinsic(
            molt_event_loop_remove_writer, "event_loop_remove_writer"
        )(self._loop_handle, fileno)
        return bool(
            _require_asyncio_intrinsic(
                molt_asyncio_fd_watcher_unregister, "asyncio_fd_watcher_unregister"
            )(self._writers, fileno)
        )

    def _run_once(self) -> int:
        """Run one iteration of the Rust event loop (hot path).

        Delegates entirely to ``molt_event_loop_run_once`` which handles
        selector poll, timer firing, and ready-queue drain in Rust.
        Returns the number of callbacks executed (0 means idle).
        """
        return int(
            _require_asyncio_intrinsic(molt_event_loop_run_once, "event_loop_run_once")(
                self._loop_handle
            )
        )

    def _has_pending(self) -> bool:
        """Return True if the Rust event loop has pending timers or I/O watchers."""
        return bool(
            _require_asyncio_intrinsic(
                molt_event_loop_has_pending, "event_loop_has_pending"
            )(self._loop_handle)
        )

    def _ready_count(self) -> int:
        """Return the number of callbacks currently in the Rust ready queue."""
        return int(
            _require_asyncio_intrinsic(
                molt_event_loop_ready_count, "event_loop_ready_count"
            )(self._loop_handle)
        )

    def _next_deadline_delay(self) -> float:
        """Return seconds until the next scheduled timer fires (inf if none)."""
        return float(
            _require_asyncio_intrinsic(
                molt_event_loop_next_deadline_delay, "event_loop_next_deadline_delay"
            )(self._loop_handle)
        )

    def _cancel_rust_timer(self, timer_id: Any) -> None:
        """Cancel a Rust-level timer by the opaque timer_id returned from call_later/call_at."""
        _require_asyncio_intrinsic(
            molt_event_loop_cancel_timer, "event_loop_cancel_timer"
        )(self._loop_handle, timer_id)

    def _notify_reader_ready(self, fd: int) -> None:
        """Notify the Rust event loop that *fd* is readable.

        Called by transport/protocol glue when the selector reports readability
        outside of the normal Rust poll path.
        """
        _require_asyncio_intrinsic(
            molt_event_loop_notify_reader_ready, "event_loop_notify_reader_ready"
        )(self._loop_handle, fd)

    def _notify_writer_ready(self, fd: int) -> None:
        """Notify the Rust event loop that *fd* is writable."""
        _require_asyncio_intrinsic(
            molt_event_loop_notify_writer_ready, "event_loop_notify_writer_ready"
        )(self._loop_handle, fd)

    def run_until_complete(self, future: Any) -> Any:
        if self.is_closed():
            raise RuntimeError("Event loop is closed")
        if self.is_running():
            raise RuntimeError("Event loop is already running")
        prev = _get_running_loop()
        _set_running_loop(self)
        # Mark the Rust handle as running.
        _require_asyncio_intrinsic(molt_event_loop_start, "event_loop_start")(
            self._loop_handle
        )
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
                        if molt_task_register_token_owned is not None:  # type: ignore[name-defined]
                            molt_task_register_token_owned(  # type: ignore[name-defined]
                                runner, fut._token.token_id()
                            )
                        molt_block_on(runner)
                        _debug_exc_state("run_until_complete_after_block_on")
                        result = fut.result()
                        _debug_exc_state("run_until_complete_after_result")
                    finally:
                        _restore_token_id(prev_token_id)
                else:
                    promise = fut._molt_promise
                    if promise is None:
                        raise RuntimeError(
                            "asyncio intrinsic not available: promise_new"
                        )
                    result = molt_block_on(promise)
                    _debug_exc_state("run_until_complete_after_promise")
            else:
                fut = Task(future, loop=self, _spawn_runner=False)
                prev_token_id = _swap_current_token(fut._token)
                try:
                    runner = fut._runner(fut.get_coro())
                    fut._runner_task = runner
                    if molt_task_register_token_owned is not None:  # type: ignore[name-defined]
                        molt_task_register_token_owned(  # type: ignore[name-defined]
                            runner, fut._token.token_id()
                        )
                    molt_block_on(runner)
                    _debug_exc_state("run_until_complete_after_block_on")
                    result = fut.result()
                    _debug_exc_state("run_until_complete_after_result")
                finally:
                    _restore_token_id(prev_token_id)
        finally:
            # Mark the Rust handle as stopped.
            _require_asyncio_intrinsic(molt_event_loop_stop, "event_loop_stop")(
                self._loop_handle
            )
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

    def add_signal_handler(
        self, sig: int, callback: Callable[..., Any], /, *args: Any
    ) -> None:
        """Register *callback* to be called when signal *sig* is received.

        On WASM platforms signals are not supported and this raises
        ``NotImplementedError``.
        """
        _platform = _sys.platform
        if _platform in ("emscripten", "wasi"):
            raise NotImplementedError(
                "signal handlers are not supported on this platform"
            )
        if not callable(callback):
            raise TypeError(f"callback must be callable, got {type(callback).__name__}")
        # Validate the signal number early.
        _signal.getsignal(sig)  # raises ValueError/OSError for invalid sigs
        self._signal_handlers[sig] = (callback, args)

        def _handle_sig(signum: int, frame: Any) -> None:
            self.call_soon_threadsafe(callback, *args)

        _signal.signal(sig, _handle_sig)

    def remove_signal_handler(self, sig: int) -> bool:
        """Remove signal handler for signal *sig*.

        Returns ``True`` if a handler was removed, ``False`` if no handler
        was installed for *sig*.

        On WASM platforms signals are not supported and this raises
        ``NotImplementedError``.
        """
        _platform = _sys.platform
        if _platform in ("emscripten", "wasi"):
            raise NotImplementedError(
                "signal handlers are not supported on this platform"
            )
        entry = self._signal_handlers.pop(sig, None)
        if entry is None:
            return False
        _signal.signal(sig, _signal.SIG_DFL)
        return True

    async def connect_read_pipe(
        self, protocol_factory: Callable[[], Protocol], pipe: Any
    ) -> tuple[Transport, Protocol]:
        """Register a read pipe in the event loop.

        *protocol_factory* is a callable returning a protocol instance.
        *pipe* is a file-like object that exposes ``fileno()``.

        Returns a ``(transport, protocol)`` tuple where *transport* is a
        :class:`_ReadPipeTransport` backed by a Rust pipe-transport intrinsic.
        """
        fileno_fn = getattr(pipe, "fileno", None)
        if not callable(fileno_fn):
            raise TypeError("pipe must have a fileno() method")
        fd = fileno_fn()
        if not isinstance(fd, int) or fd < 0:
            raise ValueError("pipe.fileno() must return a non-negative integer")
        protocol = protocol_factory()
        # Allocate the Rust-side pipe transport (read mode).
        new_fn = _require_asyncio_intrinsic(
            molt_pipe_transport_new, "pipe_transport_new"
        )
        pipe_handle = new_fn(fd, True)
        transport = _ReadPipeTransport(self, pipe, protocol, pipe_handle)
        # Notify the protocol that the connection has been established.
        connection_made = getattr(protocol, "connection_made", None)
        if callable(connection_made):
            connection_made(transport)
        # Register the fd as a reader on the event loop so that data arrival
        # triggers ``protocol.data_received`` via the ready queue.
        self.add_reader(fd, self._pipe_read_ready, transport, protocol)
        return transport, protocol

    def _pipe_read_ready(
        self, transport: _ReadPipeTransport, protocol: Protocol
    ) -> None:
        """Internal callback invoked when a read-pipe fd becomes readable."""
        if transport.is_closing():
            return
        fd_fn = _require_asyncio_intrinsic(
            molt_pipe_transport_get_fd, "pipe_transport_get_fd"
        )
        fd = fd_fn(transport._pipe_handle)
        data = _os.read(fd, 65536)
        if data:
            data_received = getattr(protocol, "data_received", None)
            if callable(data_received):
                data_received(data)
        else:
            # EOF — remove reader and notify protocol.
            self.remove_reader(fd)
            eof_received = getattr(protocol, "eof_received", None)
            keep_open = False
            if callable(eof_received):
                keep_open = bool(eof_received())
            if not keep_open:
                transport.close()

    async def connect_write_pipe(
        self, protocol_factory: Callable[[], Protocol], pipe: Any
    ) -> tuple[Transport, Protocol]:
        """Register a write pipe in the event loop.

        *protocol_factory* is a callable returning a protocol instance.
        *pipe* is a file-like object that exposes ``fileno()``.

        Returns a ``(transport, protocol)`` tuple where *transport* is a
        :class:`_WritePipeTransport` backed by a Rust pipe-transport intrinsic.
        """
        fileno_fn = getattr(pipe, "fileno", None)
        if not callable(fileno_fn):
            raise TypeError("pipe must have a fileno() method")
        fd = fileno_fn()
        if not isinstance(fd, int) or fd < 0:
            raise ValueError("pipe.fileno() must return a non-negative integer")
        protocol = protocol_factory()
        # Allocate the Rust-side pipe transport (write mode).
        new_fn = _require_asyncio_intrinsic(
            molt_pipe_transport_new, "pipe_transport_new"
        )
        pipe_handle = new_fn(fd, False)
        transport = _WritePipeTransport(self, pipe, protocol, pipe_handle)
        # Notify the protocol that the connection has been established.
        connection_made = getattr(protocol, "connection_made", None)
        if callable(connection_made):
            connection_made(transport)
        return transport, protocol

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
            getpeername_fn = getattr(sock, "getpeername", None)
            if callable(getpeername_fn):
                peer = getpeername_fn()
                if isinstance(peer, tuple) and peer:
                    host = peer[0]
                    if isinstance(host, str) and host:
                        resolved_server_hostname = host
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
            transport._closed = True
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
        loop = molt_asyncio_event_loop_get_current()
        if _TYPE_CHECKING:
            return _cast(EventLoop, loop)
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
        callback(int(pid), int(returncode), *args)


class SafeChildWatcher(AbstractChildWatcher):
    pass


class BaseChildWatcher(AbstractChildWatcher):
    pass


class FastChildWatcher(AbstractChildWatcher):
    pass


class MultiLoopChildWatcher(AbstractChildWatcher):
    pass


class ThreadedChildWatcher(AbstractChildWatcher):
    pass


class PidfdChildWatcher(AbstractChildWatcher):
    pass


_CHILD_WATCHER: AbstractChildWatcher | None = None
_CAN_USE_PIDFD_CACHE: bool | None = None


def can_use_pidfd() -> bool:
    global _CAN_USE_PIDFD_CACHE
    if _CAN_USE_PIDFD_CACHE is not None:
        return _CAN_USE_PIDFD_CACHE
    pidfd_open = getattr(_os, "pidfd_open", None)
    if pidfd_open is None:
        _CAN_USE_PIDFD_CACHE = False
        return False
    try:
        fd = int(pidfd_open(int(_os.getpid()), 0))
    except OSError:
        _CAN_USE_PIDFD_CACHE = False
        return False
    try:
        _os.close(fd)
    except OSError:
        pass
    _CAN_USE_PIDFD_CACHE = True
    return True


def waitstatus_to_exitcode(status: int) -> int:
    converter = getattr(_os, "waitstatus_to_exitcode", None)
    if converter is None:
        raise NotImplementedError("os.waitstatus_to_exitcode is unavailable")
    return int(converter(status))


def get_child_watcher() -> AbstractChildWatcher:
    _require_child_watcher_support()
    global _CHILD_WATCHER
    if _CHILD_WATCHER is None:
        _CHILD_WATCHER = ThreadedChildWatcher()
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


class BaseTransport:
    """Base class for transports."""

    def __init__(self, extra: dict | None = None):
        self._extra = extra if extra is not None else {}

    def get_extra_info(self, name: str, default: Any = None) -> Any:
        return self._extra.get(name, default)

    def is_closing(self) -> bool:
        raise NotImplementedError

    def close(self) -> None:
        raise NotImplementedError

    def set_protocol(self, protocol: "BaseProtocol") -> None:
        raise NotImplementedError

    def get_protocol(self) -> "BaseProtocol":
        raise NotImplementedError


class ReadTransport(BaseTransport):
    """Interface for read-only transports."""

    def is_reading(self) -> bool:
        raise NotImplementedError

    def pause_reading(self) -> None:
        raise NotImplementedError

    def resume_reading(self) -> None:
        raise NotImplementedError


class WriteTransport(BaseTransport):
    """Interface for write-only transports."""

    def set_write_buffer_limits(
        self, high: int | None = None, low: int | None = None
    ) -> None:
        raise NotImplementedError

    def get_write_buffer_size(self) -> int:
        raise NotImplementedError

    def get_write_buffer_limits(self) -> tuple[int, int]:
        raise NotImplementedError

    def write(self, data: bytes) -> None:
        raise NotImplementedError

    def writelines(self, list_of_data: list[bytes]) -> None:
        for data in list_of_data:
            self.write(data)

    def write_eof(self) -> None:
        raise NotImplementedError

    def can_write_eof(self) -> bool:
        raise NotImplementedError

    def abort(self) -> None:
        raise NotImplementedError


class Transport(ReadTransport, WriteTransport):
    """Interface representing a bidirectional transport."""


class DatagramTransport(BaseTransport):
    """Interface for datagram (UDP) transports."""

    def sendto(self, data: bytes, addr: Any = None) -> None:
        raise NotImplementedError

    def abort(self) -> None:
        raise NotImplementedError


class SubprocessTransport(BaseTransport):
    """Interface for subprocess transports."""

    def get_pid(self) -> int:
        raise NotImplementedError

    def get_returncode(self) -> int | None:
        raise NotImplementedError

    def get_pipe_transport(self, fd: int) -> BaseTransport | None:
        raise NotImplementedError

    def send_signal(self, signal: int) -> None:
        raise NotImplementedError

    def terminate(self) -> None:
        raise NotImplementedError

    def kill(self) -> None:
        raise NotImplementedError

    def close(self) -> None:
        raise NotImplementedError


class BaseProtocol:
    """Base class for protocols."""

    def connection_made(self, transport: BaseTransport) -> None:
        """Called when a connection is made."""

    def connection_lost(self, exc: BaseException | None) -> None:
        """Called when the connection is lost or closed."""

    def pause_writing(self) -> None:
        """Called when the transport's buffer goes over the high-water mark."""

    def resume_writing(self) -> None:
        """Called when the transport's buffer drains below the low-water mark."""


class Protocol(BaseProtocol):
    """Interface for stream protocol event callbacks."""

    def data_received(self, data: bytes) -> None:
        """Called when some data is received."""

    def eof_received(self) -> bool | None:
        """Called when the other end signals it won't send data anymore."""


class BufferedProtocol(BaseProtocol):
    """Interface for stream protocol with manual buffer control."""

    def get_buffer(self, sizehint: int) -> bytearray:
        """Called to allocate a new receive buffer."""
        raise NotImplementedError

    def buffer_updated(self, nbytes: int) -> None:
        """Called when the buffer was updated with the received data."""
        raise NotImplementedError

    def eof_received(self) -> bool | None:
        """Called when the other end signals it won't send data anymore."""


class DatagramProtocol(BaseProtocol):
    """Interface for datagram protocol event callbacks."""

    def datagram_received(self, data: bytes, addr: Any) -> None:
        """Called when a datagram is received."""

    def error_received(self, exc: OSError) -> None:
        """Called when a send or receive operation raises an OSError."""


class SubprocessProtocol(BaseProtocol):
    """Interface for subprocess event callbacks."""

    def pipe_data_received(self, fd: int, data: bytes) -> None:
        """Called when the child process writes data into its stdout or stderr pipe."""

    def pipe_connection_lost(self, fd: int, exc: BaseException | None) -> None:
        """Called when one of the pipes communicating with the child process is closed."""

    def process_exited(self) -> None:
        """Called when the child process has exited."""


class StreamReaderProtocol(Protocol):
    """Stream reader protocol."""


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
        if hasattr(self._sock, "close"):
            self._sock.close()

    def is_closing(self) -> bool:
        return self._closed

    def get_extra_info(self, name: str, default: Any = None) -> Any:
        if name == "socket":
            return self._sock
        return default


class _ReadPipeTransport(Transport):
    """Read pipe transport backed by Rust intrinsics.

    Wraps a file descriptor for reading and dispatches data to a protocol
    via the ``data_received`` / ``eof_received`` / ``connection_lost``
    callbacks.
    """

    def __init__(
        self,
        loop: "_EventLoop",
        pipe: Any,
        protocol: Protocol,
        pipe_handle: int,
    ) -> None:
        self._loop = loop
        self._pipe = pipe
        self._protocol = protocol
        self._pipe_handle = pipe_handle
        self._closing = False
        self._paused = False

    def get_extra_info(self, name: str, default: Any = None) -> Any:
        if name == "pipe":
            return self._pipe
        return default

    def is_closing(self) -> bool:
        if self._closing:
            return True
        is_closing_fn = _require_asyncio_intrinsic(
            molt_pipe_transport_is_closing, "pipe_transport_is_closing"
        )
        return bool(is_closing_fn(self._pipe_handle))

    def close(self) -> None:
        if self._closing:
            return
        self._closing = True
        close_fn = _require_asyncio_intrinsic(
            molt_pipe_transport_close, "pipe_transport_close"
        )
        close_fn(self._pipe_handle)
        connection_lost = getattr(self._protocol, "connection_lost", None)
        if callable(connection_lost):
            connection_lost(None)

    def pause_reading(self) -> None:
        if self._paused or self._closing:
            return
        self._paused = True
        pause_fn = _require_asyncio_intrinsic(
            molt_pipe_transport_pause_reading, "pipe_transport_pause_reading"
        )
        pause_fn(self._pipe_handle)

    def resume_reading(self) -> None:
        if not self._paused or self._closing:
            return
        self._paused = False
        resume_fn = _require_asyncio_intrinsic(
            molt_pipe_transport_resume_reading, "pipe_transport_resume_reading"
        )
        resume_fn(self._pipe_handle)

    def get_pid(self) -> int | None:
        return None

    def get_pipe(self) -> Any:
        return self._pipe

    def __del__(self) -> None:
        drop_fn = _require_asyncio_intrinsic(
            molt_pipe_transport_drop, "pipe_transport_drop"
        )
        drop_fn(self._pipe_handle)


class _WritePipeTransport(Transport):
    """Write pipe transport backed by Rust intrinsics.

    Wraps a file descriptor for writing and provides the ``write()`` /
    ``write_eof()`` / ``close()`` interface expected by asyncio protocols.
    """

    def __init__(
        self,
        loop: "_EventLoop",
        pipe: Any,
        protocol: Protocol,
        pipe_handle: int,
    ) -> None:
        self._loop = loop
        self._pipe = pipe
        self._protocol = protocol
        self._pipe_handle = pipe_handle
        self._closing = False

    def get_extra_info(self, name: str, default: Any = None) -> Any:
        if name == "pipe":
            return self._pipe
        return default

    def is_closing(self) -> bool:
        if self._closing:
            return True
        is_closing_fn = _require_asyncio_intrinsic(
            molt_pipe_transport_is_closing, "pipe_transport_is_closing"
        )
        return bool(is_closing_fn(self._pipe_handle))

    def write(self, data: bytes) -> None:
        if self._closing:
            raise RuntimeError("transport is closing")
        if not data:
            return
        write_fn = _require_asyncio_intrinsic(
            molt_pipe_transport_write, "pipe_transport_write"
        )
        write_fn(self._pipe_handle, data)

    def write_eof(self) -> None:
        self.close()

    def can_write_eof(self) -> bool:
        return True

    def get_write_buffer_size(self) -> int:
        buf_fn = _require_asyncio_intrinsic(
            molt_pipe_transport_get_write_buffer_size,
            "pipe_transport_get_write_buffer_size",
        )
        return int(buf_fn(self._pipe_handle))

    def close(self) -> None:
        if self._closing:
            return
        self._closing = True
        close_fn = _require_asyncio_intrinsic(
            molt_pipe_transport_close, "pipe_transport_close"
        )
        close_fn(self._pipe_handle)
        connection_lost = getattr(self._protocol, "connection_lost", None)
        if callable(connection_lost):
            connection_lost(None)

    def abort(self) -> None:
        self.close()

    def get_pid(self) -> int | None:
        return None

    def get_pipe(self) -> Any:
        return self._pipe

    def __del__(self) -> None:
        drop_fn = _require_asyncio_intrinsic(
            molt_pipe_transport_drop, "pipe_transport_drop"
        )
        drop_fn(self._pipe_handle)


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
    if _VERSION_INFO >= (3, 14):
        _warnings.warn(
            "get_event_loop_policy() is deprecated and will be removed in Python 3.16",
            DeprecationWarning,
            stacklevel=2,
        )
    policy = molt_asyncio_event_loop_policy_get()
    if policy is None:
        policy = _default_event_loop_policy()
        molt_asyncio_event_loop_policy_set(policy)
    return policy


def set_event_loop_policy(policy: AbstractEventLoopPolicy | None) -> None:
    if _VERSION_INFO >= (3, 14):
        _warnings.warn(
            "set_event_loop_policy() is deprecated and will be removed in Python 3.16",
            DeprecationWarning,
            stacklevel=2,
        )
    if policy is None:
        policy = _default_event_loop_policy()
    molt_asyncio_event_loop_policy_set(policy)


def get_event_loop() -> EventLoop:
    if _VERSION_INFO >= (3, 14):
        loop = _get_running_loop()
        if loop is not None:
            return loop
        raise RuntimeError(
            "There is no current event loop in thread %r."
            % _threading.current_thread().name
        )
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
        if context is not None:
            task = loop.create_task(coro, context=context)
        else:
            task = loop.create_task(coro)
        try:
            result = loop.run_until_complete(task)
        except BaseException:
            _cancel_all_tasks(loop)
            import sys as _aio_sys
            _aio_mod_dict = getattr(_aio_sys.modules.get(__name__), "__dict__", None) or globals()
            shutdown = _aio_mod_dict.get("molt_asyncgen_shutdown")
            if shutdown is not None:
                shutdown()
            raise
        _cancel_all_tasks(loop)
        import sys as _aio_sys
        _aio_mod_dict = getattr(_aio_sys.modules.get(__name__), "__dict__", None) or globals()
        shutdown = _aio_mod_dict.get("molt_asyncgen_shutdown")
        if shutdown is not None:
            shutdown()
        return result

    def close(self) -> None:
        if self._loop is None:
            return
        if not self._loop.is_closed():
            _cancel_all_tasks(self._loop)
            import sys as _aio_sys
            _aio_mod_dict = getattr(_aio_sys.modules.get(__name__), "__dict__", None) or globals()
            shutdown = _aio_mod_dict.get("molt_asyncgen_shutdown")
            if shutdown is not None:
                shutdown()
            self._loop.close()
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
            try:
                tls_handle = _tls_client_connect(
                    host,
                    int(port),
                    host if ssl is not False else None,
                )
            except OSError as exc:
                raise _map_socket_name_resolution_error(exc) from None
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
    args_tuple = args if args else None
    kwargs_dict = kwargs if kwargs else None
    return await molt_asyncio_to_thread(func, args_tuple, kwargs_dict)


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
    if ssl is False:
        raise TypeError("ssl argument must be an SSLContext or None")
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
    if ssl is False:
        raise TypeError("ssl argument must be an SSLContext or None")
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


class Queue:
    _Q_TYPE: int = 0  # FIFO

    def __init__(self, maxsize: int = 0) -> None:
        if maxsize < 0:
            raise ValueError("maxsize must be >= 0")
        self._maxsize = maxsize
        self._q_handle: int = molt_asyncio_queue_new(maxsize, self._Q_TYPE)
        # Waiter lists live in the Rust handle (getter/putter VecDeques).
        # Do NOT create Python-side _getters/_putters deques here.
        self._unfinished_tasks = 0
        self._finished = Event()
        self._finished.set()
        self._shutdown = False
        self._init()

    @property
    def maxsize(self) -> int:
        return self._maxsize

    def _init(self) -> None:
        self._queue: Any = _deque()

    def qsize(self) -> int:
        return int(molt_asyncio_queue_qsize(self._q_handle))

    def _handle_maxsize(self) -> int:
        return int(molt_asyncio_queue_maxsize(self._q_handle))

    def empty(self) -> bool:
        return bool(molt_asyncio_queue_empty(self._q_handle))

    def full(self) -> bool:
        return bool(molt_asyncio_queue_full(self._q_handle))

    async def put(self, item: Any) -> None:
        if molt_asyncio_queue_is_shutdown(self._q_handle):
            raise _QueueShutDown
        while self.full():
            fut = Future()
            molt_asyncio_queue_add_putter(self._q_handle, fut)
            try:
                await fut
            except BaseException as exc:
                if _is_cancelled_exc(exc):
                    # Release Rust's reference to this waiter on cancellation.
                    molt_asyncio_queue_notify_putters(self._q_handle, 1)
                raise
            if molt_asyncio_queue_is_shutdown(self._q_handle):
                raise _QueueShutDown
        self._put_nowait(item)

    def put_nowait(self, item: Any) -> None:
        if molt_asyncio_queue_is_shutdown(self._q_handle):
            raise _QueueShutDown
        if self.full():
            raise QueueFull
        self._put_nowait(item)

    def _put_nowait(self, item: Any) -> None:
        self._unfinished_tasks += 1
        molt_asyncio_queue_put_nowait(self._q_handle, item)
        if self._finished.is_set():
            self._finished.clear()
        if int(molt_asyncio_queue_getter_count(self._q_handle)) > 0:
            molt_asyncio_queue_notify_getters(self._q_handle, 1)
        else:
            self._put(item)

    def _put(self, item: Any) -> None:
        self._queue.append(item)

    async def get(self) -> Any:
        if not molt_asyncio_queue_empty(self._q_handle):
            return self._get_nowait()
        if molt_asyncio_queue_is_shutdown(self._q_handle):
            raise _QueueShutDown
        fut = Future()
        molt_asyncio_queue_add_getter(self._q_handle, fut)
        try:
            return await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                # Release Rust's reference to this waiter on cancellation.
                molt_asyncio_queue_notify_getters(self._q_handle, 1)
            raise

    def get_nowait(self) -> Any:
        if not molt_asyncio_queue_empty(self._q_handle):
            return self._get_nowait()
        if molt_asyncio_queue_is_shutdown(self._q_handle):
            raise _QueueShutDown
        raise QueueEmpty

    def _get_nowait(self) -> Any:
        item = self._get()
        molt_asyncio_queue_get_nowait(self._q_handle)
        if int(molt_asyncio_queue_putter_count(self._q_handle)) > 0:
            molt_asyncio_queue_notify_putters(self._q_handle, 1)
        return item

    def _get(self) -> Any:
        return self._queue.popleft()

    def task_done(self) -> None:
        if self._unfinished_tasks <= 0:
            raise ValueError("task_done() called too many times")
        self._unfinished_tasks -= 1
        molt_asyncio_queue_task_done(self._q_handle)
        if self._unfinished_tasks == 0:
            self._finished.set()

    async def join(self) -> None:
        await self._finished.wait()

    if _EXPOSE_QUEUE_SHUTDOWN:

        def shutdown(self) -> None:
            self._shutdown = True
            molt_asyncio_queue_shutdown(self._q_handle, False)
            n_getters = int(molt_asyncio_queue_getter_count(self._q_handle))
            if n_getters > 0:
                molt_asyncio_queue_notify_getters(self._q_handle, n_getters)
            n_putters = int(molt_asyncio_queue_putter_count(self._q_handle))
            if n_putters > 0:
                molt_asyncio_queue_notify_putters(self._q_handle, n_putters)

    def __repr__(self) -> str:
        size = self.qsize()
        maxsize = self._maxsize
        if maxsize > 0:
            return f"<Queue maxsize={maxsize} qsize={size}>"
        return f"<Queue qsize={size}>"

    @classmethod
    def __class_getitem__(cls, item: Any) -> Any:
        return _require_asyncio_intrinsic(molt_generic_alias_new, "generic_alias_new")(
            cls, item
        )

    def __del__(self) -> None:
        handle = getattr(self, "_q_handle", None)
        if handle is not None:
            molt_asyncio_queue_drop(handle)


class PriorityQueue(Queue):
    _Q_TYPE: int = 2  # Priority

    def _init(self) -> None:
        self._queue = []

    def _put(self, item: Any) -> None:
        _heapq.heappush(self._queue, item)

    def _get(self) -> Any:
        return _heapq.heappop(self._queue)


class LifoQueue(Queue):
    _Q_TYPE: int = 1  # LIFO

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
            setattr(mod, key, val)
    mod.__name__ = name
    mod.__package__ = name.rpartition(".")[0]
    _sys.modules[name] = mod
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


def on_fork() -> None:
    global _CHILD_WATCHER
    _set_running_loop(None)
    _CHILD_WATCHER = None


events = _module(
    "asyncio.events",
    {
        "AbstractEventLoop": AbstractEventLoop,
        "AbstractEventLoopPolicy": AbstractEventLoopPolicy,
        "AbstractServer": AbstractServer,
        "BaseDefaultEventLoopPolicy": DefaultEventLoopPolicy,
        "Handle": Handle,
        "TimerHandle": TimerHandle,
        "contextvars": _contextvars,
        "get_child_watcher": get_child_watcher,
        "get_event_loop": get_event_loop,
        "get_event_loop_policy": get_event_loop_policy,
        "get_running_loop": get_running_loop,
        "new_event_loop": new_event_loop,
        "on_fork": on_fork,
        "os": _os,
        "set_child_watcher": set_child_watcher,
        "set_event_loop": set_event_loop,
        "set_event_loop_policy": set_event_loop_policy,
        "signal": _signal,
        "socket": _socket,
        "subprocess": _subprocess,
        "sys": _sys,
        "threading": _threading,
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
        "enum": _enum,
        "LOG_THRESHOLD_FOR_CONNLOST_WRITES": 5,
        "ACCEPT_RETRY_DELAY": 1,
        "DEBUG_STACK_DEPTH": 10,
        "SSL_HANDSHAKE_TIMEOUT": 60.0,
        "SSL_SHUTDOWN_TIMEOUT": 30.0,
        "SENDFILE_FALLBACK_READBUFFER_SIZE": 256 * 1024,
        "FLOW_CONTROL_HIGH_WATER_SSL_READ": 256,
        "FLOW_CONTROL_HIGH_WATER_SSL_WRITE": 512,
        "THREAD_JOIN_TIMEOUT": 300,
    },
)

coroutines = _module(
    "asyncio.coroutines",
    {
        "collections": _collections,
        "inspect": _inspect,
        "iscoroutine": iscoroutine,
        "iscoroutinefunction": iscoroutinefunction,
        "os": _os,
        "sys": _sys,
        "types": _types,
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
        "BrokenBarrierError": BrokenBarrierError,
    },
)

format_helpers = _module(
    "asyncio.format_helpers",
    {
        "constants": constants,
        "_format_callback_source": _format_callback_source,
        "extract_stack": _extract_stack,
        "functools": _functools,
        "inspect": _inspect,
        "reprlib": _reprlib,
        "sys": _sys,
        "traceback": _traceback,
    },
)
setattr(events, "format_helpers", format_helpers)


def _queues_attrs() -> dict[str, Any]:
    attrs = {
        "GenericAlias": _types.GenericAlias,
        "Queue": Queue,
        "PriorityQueue": PriorityQueue,
        "LifoQueue": LifoQueue,
        "QueueEmpty": QueueEmpty,
        "QueueFull": QueueFull,
        "collections": _collections,
        "heapq": _heapq,
        "locks": locks,
        "mixins": mixins,
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
            "logging": _logging,
        },
    )


def _make_mixins_module() -> _types.ModuleType:
    return _module(
        "asyncio.mixins",
        {
            "events": events,
            "threading": _threading,
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
            "collections": _collections,
            "enum": _enum,
            "exceptions": exceptions,
            "mixins": mixins,
        },
    )


def _make_queues_module() -> _types.ModuleType:
    return _module("asyncio.queues", _queues_attrs())


log = _make_log_module()
mixins = _make_mixins_module()
locks = _make_locks_module()
queues = _make_queues_module()


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
    _mod_dict[name] = mod
    return mod


protocols = _module(
    "asyncio.protocols",
    {
        "BaseProtocol": BaseProtocol,
        "Protocol": Protocol,
        "BufferedProtocol": BufferedProtocol,
        "DatagramProtocol": DatagramProtocol,
        "SubprocessProtocol": SubprocessProtocol,
    },
)

transports = _module(
    "asyncio.transports",
    {
        "BaseTransport": Transport,
        "Transport": Transport,
        "ReadTransport": Transport,
        "WriteTransport": Transport,
        "DatagramTransport": DatagramTransport,
        "SubprocessTransport": SubprocessTransport,
    },
)

runners = _module(
    "asyncio.runners",
    {
        "Runner": Runner,
        "constants": constants,
        "contextvars": _contextvars,
        "coroutines": coroutines,
        "enum": _enum,
        "events": events,
        "exceptions": exceptions,
        "functools": _functools,
        "run": run,
        "signal": _signal,
        "threading": _threading,
    },
)

taskgroups = _module(
    "asyncio.taskgroups",
    {
        "TaskGroup": TaskGroup,
        "events": events,
        "exceptions": exceptions,
    },
)

threads = _module(
    "asyncio.threads",
    {
        "contextvars": _contextvars,
        "events": events,
        "functools": _functools,
        "to_thread": to_thread,
    },
)

_typing_type_alias = getattr(typing, "Type", None)
if _typing_type_alias is None:

    class _SpecialGenericAlias:
        def __call__(self, *args: Any, **kwargs: Any) -> None:
            return None

    _typing_type_alias = _SpecialGenericAlias()

_typing_optional_alias = getattr(typing, "Optional", None)
if _typing_optional_alias is None or not callable(_typing_optional_alias):

    class _SpecialForm:
        def __call__(self, *args: Any, **kwargs: Any) -> None:
            return None

    _typing_optional_alias = _SpecialForm()

_typing_final_alias = getattr(typing, "final", None)
if _typing_final_alias is None:

    def _typing_final_alias(arg: Any) -> Any:
        return arg


timeouts = _module(
    "asyncio.timeouts",
    {
        "Optional": _typing_optional_alias,
        "Timeout": _Timeout,
        "TracebackType": _types.TracebackType,
        "Type": _typing_type_alias,
        "enum": _enum,
        "events": events,
        "exceptions": exceptions,
        "final": _typing_final_alias,
        "timeout": timeout,
        "timeout_at": timeout_at,
    },
)

base_futures = _module(
    "asyncio.base_futures",
    {
        "format_helpers": format_helpers,
        "isfuture": isfuture,
        "reprlib": _reprlib,
    },
)

base_tasks = _module(
    "asyncio.base_tasks",
    {
        "base_futures": base_futures,
        "coroutines": coroutines,
        "linecache": _linecache,
        "reprlib": _reprlib,
        "traceback": _traceback,
    },
)

base_subprocess = _module(
    "asyncio.base_subprocess",
    {
        "BaseSubprocessTransport": Process,
        "ReadSubprocessPipeProto": Process,
        "WriteSubprocessPipeProto": Process,
        "collections": _collections,
        "logger": _logging.getLogger("asyncio"),
        "protocols": protocols,
        "subprocess": _subprocess,
        "transports": transports,
        "warnings": _warnings,
    },
)

_SC_IOV_MAX = 1024
if hasattr(_os, "sysconf"):
    _sysconf_val = _os.sysconf("SC_IOV_MAX")
    if isinstance(_sysconf_val, int) and _sysconf_val > 0:
        _SC_IOV_MAX = _sysconf_val

selector_events = _module(
    "asyncio.selector_events",
    {
        "BaseSelectorEventLoop": SelectorEventLoop,
        "SC_IOV_MAX": _SC_IOV_MAX,
        "base_events": base_events,
        "collections": _collections,
        "constants": constants,
        "errno": _errno,
        "events": events,
        "functools": _functools,
        "futures": base_futures,
        "itertools": _itertools,
        "logger": _logging.getLogger("asyncio"),
        "os": _os,
        "protocols": protocols,
        "selectors": _selectors,
        "socket": _socket,
        "ssl": _ssl,
        "transports": transports,
        "warnings": _warnings,
        "weakref": _weakref,
    },
)


class EnumType(type):
    pass


class AppProtocolState(metaclass=EnumType):
    STATE_INIT = 0
    STATE_CON_MADE = 1
    STATE_EOF = 2
    STATE_CON_LOST = 3


class SSLProtocolState(metaclass=EnumType):
    UNWRAPPED = 0
    DO_HANDSHAKE = 1
    WRAPPED = 2
    FLUSHING = 3
    SHUTDOWN = 4


class SSLProtocol:
    pass


del EnumType


_ssl_again_errors: list[type[Any]] = []
for _name in ("SSLWantReadError", "SSLWantWriteError"):
    _value = getattr(_ssl, _name, None)
    if isinstance(_value, type):
        _ssl_again_errors.append(_value)
if not _ssl_again_errors:
    _ssl_again_errors = [_ssl.SSLError]
SSLAgainErrors = tuple(_ssl_again_errors)


def add_flowcontrol_defaults(
    *args: Any, **kwargs: Any
) -> tuple[tuple[Any, ...], dict[str, Any]]:
    return args, kwargs


sslproto = _module(
    "asyncio.sslproto",
    {
        "AppProtocolState": AppProtocolState,
        "SSLAgainErrors": SSLAgainErrors,
        "SSLProtocol": SSLProtocol,
        "SSLProtocolState": SSLProtocolState,
        "add_flowcontrol_defaults": add_flowcontrol_defaults,
        "collections": _collections,
        "constants": constants,
        "enum": _enum,
        "exceptions": exceptions,
        "logger": _logging.getLogger("asyncio"),
        "protocols": protocols,
        "ssl": _ssl,
        "transports": transports,
        "warnings": _warnings,
    },
)
setattr(selector_events, "sslproto", sslproto)

subprocess = _module(
    "asyncio.subprocess",
    {
        "PIPE": _SubprocessConstants.PIPE,
        "STDOUT": _SubprocessConstants.STDOUT,
        "DEVNULL": _SubprocessConstants.DEVNULL,
        "Process": Process,
        "SubprocessStreamProtocol": StreamReaderProtocol,
        "create_subprocess_exec": create_subprocess_exec,
        "create_subprocess_shell": create_subprocess_shell,
        "events": events,
        "logger": _logging.getLogger("asyncio"),
        "protocols": protocols,
        "subprocess": _subprocess,
    },
)
_futures_attrs: dict[str, Any] = {
    "Future": Future,
    "GenericAlias": _types.GenericAlias,
    "STACK_DEBUG": 0,
    "base_futures": base_futures,
    "concurrent": _concurrent,
    "contextvars": _contextvars,
    "events": events,
    "exceptions": exceptions,
    "format_helpers": format_helpers,
    "isfuture": isfuture,
    "logging": _logging,
    "sys": _sys,
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
setattr(selector_events, "futures", futures)

tasks = _module(
    "asyncio.tasks",
    {
        "ALL_COMPLETED": "ALL_COMPLETED",
        "FIRST_COMPLETED": "FIRST_COMPLETED",
        "FIRST_EXCEPTION": "FIRST_EXCEPTION",
        "GenericAlias": _types.GenericAlias,
        "Task": Task,
        "all_tasks": all_tasks,
        "as_completed": as_completed,
        "base_tasks": base_tasks,
        "concurrent": _concurrent,
        "contextvars": _contextvars,
        "coroutines": coroutines,
        "create_eager_task_factory": create_eager_task_factory,
        "create_task": create_task,
        "current_task": current_task,
        "eager_task_factory": eager_task_factory,
        "ensure_future": ensure_future,
        "events": events,
        "exceptions": exceptions,
        "functools": _functools,
        "futures": futures,
        "gather": gather,
        "inspect": _inspect,
        "itertools": _itertools,
        "run_coroutine_threadsafe": run_coroutine_threadsafe,
        "shield": shield,
        "sleep": sleep,
        "timeouts": timeouts,
        "types": _types,
        "wait": wait,
        "wait_for": wait_for,
        "warnings": _warnings,
        "weakref": _weakref,
    },
)
setattr(runners, "tasks", tasks)
setattr(timeouts, "tasks", tasks)
setattr(taskgroups, "tasks", tasks)
setattr(subprocess, "tasks", tasks)

streams = _module(
    "asyncio.streams",
    {
        "FlowControlMixin": FlowControlMixin,
        "StreamReader": StreamReader,
        "StreamReaderProtocol": StreamReaderProtocol,
        "StreamWriter": StreamWriter,
        "collections": _collections,
        "coroutines": coroutines,
        "events": events,
        "exceptions": exceptions,
        "format_helpers": format_helpers,
        "logger": _logging.getLogger("asyncio"),
        "open_connection": open_connection,
        "open_unix_connection": open_unix_connection,
        "protocols": protocols,
        "sleep": sleep,
        "socket": _socket,
        "start_server": start_server,
        "start_unix_server": start_unix_server,
        "sys": _sys,
        "warnings": _warnings,
        "weakref": _weakref,
    },
)
setattr(subprocess, "streams", streams)


class TransportSocket:
    pass


trsock = _module(
    "asyncio.trsock",
    {
        "TransportSocket": TransportSocket,
        "socket": _socket,
    },
)
setattr(selector_events, "trsock", trsock)

if not _IS_WINDOWS:
    _unix_events_attrs: dict[str, Any] = {
        "DefaultEventLoopPolicy": _UnixDefaultEventLoopPolicy,
        "SelectorEventLoop": SelectorEventLoop,
        "base_events": base_events,
        "base_subprocess": base_subprocess,
        "can_use_pidfd": can_use_pidfd,
        "constants": constants,
        "coroutines": coroutines,
        "errno": _errno,
        "events": events,
        "exceptions": exceptions,
        "futures": futures,
        "io": _io,
        "itertools": _itertools,
        "logger": _logging.getLogger("asyncio"),
        "os": _os,
        "selector_events": selector_events,
        "selectors": _selectors,
        "signal": _signal,
        "socket": _socket,
        "stat": _stat,
        "subprocess": _subprocess,
        "sys": _sys,
        "tasks": tasks,
        "threading": _threading,
        "transports": transports,
        "waitstatus_to_exitcode": waitstatus_to_exitcode,
        "warnings": _warnings,
    }
    if _EXPOSE_CHILD_WATCHERS:
        _unix_events_attrs.update(
            {
                "__all__": (
                    "SelectorEventLoop",
                    "AbstractChildWatcher",
                    "SafeChildWatcher",
                    "FastChildWatcher",
                    "PidfdChildWatcher",
                    "MultiLoopChildWatcher",
                    "ThreadedChildWatcher",
                    "DefaultEventLoopPolicy",
                ),
                "AbstractChildWatcher": AbstractChildWatcher,
                "BaseChildWatcher": BaseChildWatcher,
                "FastChildWatcher": FastChildWatcher,
                "MultiLoopChildWatcher": MultiLoopChildWatcher,
                "PidfdChildWatcher": PidfdChildWatcher,
                "SafeChildWatcher": SafeChildWatcher,
                "ThreadedChildWatcher": ThreadedChildWatcher,
            }
        )
    else:
        _unix_events_attrs["__all__"] = (
            "SelectorEventLoop",
            "DefaultEventLoopPolicy",
        )
    unix_events = _module("asyncio.unix_events", _unix_events_attrs)
    if _EXPOSE_EVENT_LOOP:
        setattr(unix_events, "EventLoop", SelectorEventLoop)

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
        setattr(windows_events, "EventLoop", _ProactorEventLoop)
    windows_utils = _module(
        "asyncio.windows_utils",
        {
            "BUFSIZE": 8192,
            "PIPE": _subprocess.PIPE,
            "STDOUT": _subprocess.STDOUT,
            "DEVNULL": _subprocess.DEVNULL,
        },
    )


async def staggered_race(
    coro_fns: Iterable[Any], delay: float | None
) -> tuple[Any, int | None, list[Any]]:
    """Run coroutines with staggered start times and take the first to finish.

    This method takes an iterable of coroutine functions. The first one is
    started immediately. From then on, whenever the immediately preceding one
    fails (raises an exception), or when *delay* seconds has passed, the next
    coroutine is started. This continues until one of the coroutines complete
    successfully, in which case all others are cancelled, or until all
    coroutines fail.

    Args:
        coro_fns: an iterable of coroutine functions, i.e. callables that
            return a coroutine object when called. Use ``functools.partial`` or
            lambdas to pass arguments.

        delay: amount of time, in seconds, between starting coroutines. If
            ``None``, the coroutines will run sequentially.

    Returns:
        tuple *(winner_result, winner_index, exceptions)* where

        - *winner_result*: the result of the winning coroutine, or ``None``
          if no coroutines won.

        - *winner_index*: the index of the winning coroutine in
          ``coro_fns``, or ``None`` if no coroutines won. If the winning
          coroutine may return None on success, *winner_index* can be used
          to definitively determine whether any coroutine won.

        - *exceptions*: list of exceptions returned by the coroutines.
          ``len(exceptions)`` is equal to the number of coroutines actually
          started, and the order is the same as in ``coro_fns``. The winning
          coroutine's entry is ``None``.
    """
    loop = get_running_loop()
    enum_coro_fns = enumerate(coro_fns)

    # Use list cells as mutable containers to avoid nonlocal across async
    # nested function boundaries (Molt compiler constraint).
    winner_result_cell: list[Any] = [None]
    winner_index_cell: list[int | None] = [None]
    exceptions: list[Any] = []
    running_tasks: set[Task] = set()
    # Single-element list so task_done callback can read/write it.
    on_completed_fut_cell: list[Future | None] = [None]

    def task_done(task: Task) -> None:
        running_tasks.discard(task)
        on_completed_fut = on_completed_fut_cell[0]
        if (
            on_completed_fut is not None
            and not on_completed_fut.done()
            and not running_tasks
        ):
            on_completed_fut.set_result(None)

        if task.cancelled():
            return

        exc = task.exception()
        if exc is not None:
            # Unhandled exception from run_one_coro itself (programming error).
            # Surfaced via ExceptionGroup after the loop if __debug__.
            pass

    async def run_one_coro(ok_to_start: Event, previous_failed: Event | None) -> None:
        # In eager tasks this waits for the calling task to append this task
        # to running_tasks; in regular tasks this is a no-op.  See gh-124309.
        await ok_to_start.wait()

        # Wait for the previous task to finish, or for delay seconds.
        if previous_failed is not None:
            if delay is None:
                await previous_failed.wait()
            else:
                try:
                    await wait_for(previous_failed.wait(), delay)
                except TimeoutError:
                    pass

        # Get the next coroutine to run.
        try:
            this_index, coro_fn = next(enum_coro_fns)
        except StopIteration:
            return

        # Start task that will run the next coroutine.
        this_failed: Event = Event()
        next_ok_to_start: Event = Event()
        next_task: Task = loop.create_task(run_one_coro(next_ok_to_start, this_failed))
        running_tasks.add(next_task)
        next_task.add_done_callback(task_done)
        # next_task has been appended to running_tasks so it is ok to start.
        next_ok_to_start.set()

        # Prepare place to record this coroutine's exception if it loses.
        exceptions.append(None)

        try:
            result = await coro_fn()
        except (SystemExit, KeyboardInterrupt):
            raise
        except BaseException as exc:
            exceptions[this_index] = exc
            this_failed.set()  # Kickstart the next coroutine.
        else:
            # Store winner's results.
            winner_index_cell[0] = this_index
            winner_result_cell[0] = result
            # Cancel all other tasks.  We deliberately exclude the current task
            # to avoid the corner case described in https://bugs.python.org/issue30048.
            current = current_task(loop)
            for t in running_tasks:
                if t is not current:
                    t.cancel()

    propagate_cancellation_error: CancelledError | None = None
    try:
        ok_to_start: Event = Event()
        first_task: Task = loop.create_task(run_one_coro(ok_to_start, None))
        running_tasks.add(first_task)
        first_task.add_done_callback(task_done)
        # first_task has been appended to running_tasks so it is ok to start.
        ok_to_start.set()

        while running_tasks:
            fut: Future = loop.create_future()
            on_completed_fut_cell[0] = fut
            try:
                await fut
            except CancelledError as ex:
                propagate_cancellation_error = ex
                for task in running_tasks:
                    task.cancel(*ex.args)
            on_completed_fut_cell[0] = None

        if propagate_cancellation_error is not None:
            raise propagate_cancellation_error
        return winner_result_cell[0], winner_index_cell[0], exceptions
    finally:
        # Molt uses reference counting; explicit del of exceptions/
        # propagate_cancellation_error is not needed to break cycles.
        # Clear the future-cell reference so done callbacks stop firing
        # after staggered_race exits.
        on_completed_fut_cell[0] = None


staggered = _module(
    "asyncio.staggered",
    {
        "contextlib": _contextlib,
        "events": events,
        "exceptions_mod": exceptions,
        "locks": locks,
        "staggered_race": staggered_race,
        "tasks": tasks,
    },
)

for _name in ("AbstractServer", "SelectorEventLoop", "Handle", "TimerHandle"):
    if hasattr(base_events, _name):
        delattr(base_events, _name)
for _name, _value in {
    "MAXIMUM_SELECT_TIMEOUT": 24 * 3600,
    "collections": _collections,
    "concurrent": _concurrent,
    "constants": constants,
    "coroutines": coroutines,
    "errno": _errno,
    "events": events,
    "exceptions": exceptions,
    "futures": futures,
    "heapq": _heapq,
    "itertools": _itertools,
    "logger": _logging.getLogger("asyncio"),
    "os": _os,
    "protocols": protocols,
    "socket": _socket,
    "ssl": _ssl,
    "sslproto": sslproto,
    "staggered": staggered,
    "stat": _stat,
    "subprocess": _subprocess,
    "sys": _sys,
    "tasks": tasks,
    "threading": _threading,
    "time": _time,
    "timeouts": timeouts,
    "traceback": _traceback,
    "transports": transports,
    "trsock": trsock,
    "warnings": _warnings,
    "weakref": _weakref,
}.items():
    setattr(base_events, _name, _value)

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
    tools = _module(
        "asyncio.tools",
        {
            "capture_call_graph": capture_call_graph,
            "format_call_graph": format_call_graph,
            "print_call_graph": print_call_graph,
        },
    )

if not _EXPOSE_EVENT_LOOP:
    if "EventLoop" in _mod_dict:
        del _mod_dict["EventLoop"]

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
        if _name in _mod_dict:
            del _mod_dict[_name]

_builtin_targets = [
    _get_running_loop,
    _set_running_loop,
    get_running_loop,
    get_event_loop,
    current_task,
]
if _EXPOSE_GRAPH:
    _builtin_targets.extend([future_add_to_awaited_by, future_discard_from_awaited_by])
for _fn in _builtin_targets:
    _mark_builtin(_fn)


# ---------------------------------------------------------------------------
# Namespace cleanup — remove names that are not part of CPython's asyncio API.
# ---------------------------------------------------------------------------
# Preserve private aliases for names still needed at call-time inside methods.
_TYPE_CHECKING = TYPE_CHECKING
_cast = cast
for _name in (
    "TYPE_CHECKING",
    "Any",
    "Callable",
    "Iterable",
    "Iterator",
    "cast",
    "dataclass",
    "typing",
):
    import sys as _aio_cleanup_sys
    _aio_cleanup_dict = getattr(_aio_cleanup_sys.modules.get(__name__), "__dict__", None) or globals()
    _aio_cleanup_dict.pop(_name, None)
