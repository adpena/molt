"""Capability-gated asyncio shim for Molt."""

from __future__ import annotations
from typing import TYPE_CHECKING, Any, Callable, Iterable, cast
import heapq as _heapq
import logging as _logging
import os as _os
import sys as _sys
import time as _time
import traceback as _traceback
import contextlib as _contextlib
import inspect as _inspect
import errno as _errno
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

_SOCKET_MODULE: Any | None = None

class _LazySocketModule(_types.ModuleType):
    def __init__(self) -> None:
        super().__init__("socket")

    def __getattr__(self, name: str) -> Any:
        return getattr(_socket_module(), name)

    def __dir__(self) -> list[str]:
        return dir(_socket_module())

def _socket_module() -> Any:
    global _SOCKET_MODULE
    module = _SOCKET_MODULE
    if module is None:
        module = __import__("socket")
        _SOCKET_MODULE = module
    return module

_SOCKET = _LazySocketModule()
_socket = _SOCKET

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

from . import exceptions as exceptions
from .exceptions import (
    BrokenBarrierError as BrokenBarrierError,
    CancelledError as CancelledError,
    IncompleteReadError as IncompleteReadError,
    InvalidStateError as InvalidStateError,
    LimitOverrunError as LimitOverrunError,
    SendfileNotAvailableError as SendfileNotAvailableError,
    TimeoutError as TimeoutError,
)

def _is_cancelled_exc(exc: BaseException) -> bool:
    if isinstance(exc, CancelledError):
        return True
    return type(exc).__name__ == "CancelledError"

def iscoroutine(obj: Any) -> bool:
    return bool(_molt_inspect_iscoroutine(obj))

def iscoroutinefunction(func: Any) -> bool:
    return bool(_molt_inspect_iscoroutinefunction(func))

from ._debug import (
    _debug_asyncio_condition_enabled as _debug_asyncio_condition_enabled,
    _debug_asyncio_exc_enabled as _debug_asyncio_exc_enabled,
    _debug_asyncio_handles_enabled as _debug_asyncio_handles_enabled,
    _debug_asyncio_promise_enabled as _debug_asyncio_promise_enabled,
    _debug_asyncio_shutdown_enabled as _debug_asyncio_shutdown_enabled,
    _debug_exc_state as _debug_exc_state,
    _debug_gather_enabled as _debug_gather_enabled,
    _debug_task_summary as _debug_task_summary,
    _debug_tasks_enabled as _debug_tasks_enabled,
    _debug_wait_for_enabled as _debug_wait_for_enabled,
    _debug_write as _debug_write,
)

_DEBUG_GATHER = _debug_gather_enabled()

_DEBUG_WAIT_FOR = _debug_wait_for_enabled()

_DEBUG_TASKS = _debug_tasks_enabled()

_DEBUG_ASYNCIO_PROMISE = _debug_asyncio_promise_enabled()

_DEBUG_ASYNCIO_EXC = _debug_asyncio_exc_enabled()

_DEBUG_ASYNCIO_CONDITION = _debug_asyncio_condition_enabled()

_DEBUG_ASYNCIO_HANDLES = _debug_asyncio_handles_enabled()

_DEBUG_ASYNCIO_SHUTDOWN = _debug_asyncio_shutdown_enabled()

_UNSET = object()
# Upper bound (seconds) on how long ``run_forever`` blocks between turns when a
# timer is scheduled further out, so a ``stop()``/wakeup arriving from another
# thread is observed promptly instead of waiting out a long deadline. Idle waits
# with no scheduled timer also use this bound, keeping the idle loop blocking
# (never busy-spinning) while staying responsive to cross-thread wakeups.
_RUN_FOREVER_IDLE_CAP = 0.05
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
    socket_module = _socket_module()
    for name in dir(socket_module):
        if not name.startswith("EAI_"):
            continue
        value = getattr(socket_module, name, None)
        if isinstance(value, int):
            codes.add(value)
    return codes

_SOCKET_EAI_CODES: set[int] | None = None

def _map_socket_name_resolution_error(exc: OSError) -> OSError:
    global _SOCKET_EAI_CODES
    if _SOCKET_EAI_CODES is None:
        _SOCKET_EAI_CODES = _socket_eai_codes()
    errno_value = getattr(exc, "errno", None)
    if isinstance(errno_value, int) and errno_value in _SOCKET_EAI_CODES:
        gaierror_cls = getattr(_socket_module(), "gaierror", None)
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
molt_asyncio_task_last_exception_clear = _intrinsic_require(
    "molt_asyncio_task_last_exception_clear", globals()
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

_PENDING_SENTINEL: Any | None = None

def _pending_sentinel() -> Any:
    global _PENDING_SENTINEL
    if _PENDING_SENTINEL is None:
        _PENDING_SENTINEL = molt_pending()
    return _PENDING_SENTINEL

def _is_pending(value: Any) -> bool:
    pending = _pending_sentinel()
    return value is pending or value == pending

from . import futures as futures
from .futures import (
    Future as Future,
    future_add_to_awaited_by as future_add_to_awaited_by,
    future_discard_from_awaited_by as future_discard_from_awaited_by,
    isfuture as isfuture,
)

class _SubprocessConstants:
    PIPE = _SUBPROCESS_PIPE
    DEVNULL = _SUBPROCESS_DEVNULL
    STDOUT = _SUBPROCESS_STDOUT

_SUBPROCESS_CONSTANTS = _SubprocessConstants

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
            _SUBPROCESS_CONSTANTS.PIPE,
            _SUBPROCESS_CONSTANTS.DEVNULL,
            _SUBPROCESS_CONSTANTS.STDOUT,
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

from . import tasks as tasks
from .tasks import (
    ALL_COMPLETED as ALL_COMPLETED,
    FIRST_COMPLETED as FIRST_COMPLETED,
    FIRST_EXCEPTION as FIRST_EXCEPTION,
    CancellationToken as CancellationToken,
    FrameCallGraphEntry as FrameCallGraphEntry,
    FutureCallGraph as FutureCallGraph,
    Runner as Runner,
    Task as Task,
    TaskGroup as TaskGroup,
    _AsCompletedIterator as _AsCompletedIterator,
    _Timeout as _Timeout,
    _cleanup_event_waiters_for_token as _cleanup_event_waiters_for_token,
    _current_token_id as _current_token_id,
    _future_cancelled as _future_cancelled,
    _future_done as _future_done,
    _future_exception as _future_exception,
    _next_task_name as _next_task_name,
    _register_event_waiter as _register_event_waiter,
    _restore_token_id as _restore_token_id,
    _swap_current_token as _swap_current_token,
    _unregister_event_waiter as _unregister_event_waiter,
    _wait_one as _wait_one,
    all_tasks as all_tasks,
    as_completed as as_completed,
    capture_call_graph as capture_call_graph,
    create_eager_task_factory as create_eager_task_factory,
    create_task as create_task,
    current_task as current_task,
    eager_task_factory as eager_task_factory,
    ensure_future as ensure_future,
    format_call_graph as format_call_graph,
    gather as gather,
    print_call_graph as print_call_graph,
    run as run,
    run_coroutine_threadsafe as run_coroutine_threadsafe,
    shield as shield,
    sleep as sleep,
    spawn as spawn,
    timeout as timeout,
    timeout_at as timeout_at,
    to_thread as to_thread,
    wait as wait,
    wait_for as wait_for,
    wrap_future as wrap_future,
)

from . import locks as locks
from .locks import (
    Barrier as Barrier,
    BoundedSemaphore as BoundedSemaphore,
    Condition as Condition,
    Event as Event,
    Lock as Lock,
    Semaphore as Semaphore,
)
from . import queues as queues
from .queues import (
    LifoQueue as LifoQueue,
    PriorityQueue as PriorityQueue,
    Queue as Queue,
    QueueEmpty as QueueEmpty,
    QueueFull as QueueFull,
)
if _EXPOSE_QUEUE_SHUTDOWN:
    from .queues import QueueShutDown as QueueShutDown

from . import transports as transports
from .transports import (
    BaseTransport as BaseTransport,
    DatagramTransport as DatagramTransport,
    ReadTransport as ReadTransport,
    SubprocessTransport as SubprocessTransport,
    Transport as Transport,
    WriteTransport as WriteTransport,
)
from . import protocols as protocols
from .protocols import (
    BaseProtocol as BaseProtocol,
    BufferedProtocol as BufferedProtocol,
    DatagramProtocol as DatagramProtocol,
    Protocol as Protocol,
    SubprocessProtocol as SubprocessProtocol,
)

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

from . import streams as streams
from .streams import (
    AbstractServer as AbstractServer,
    ProcessStreamReader as ProcessStreamReader,
    ProcessStreamWriter as ProcessStreamWriter,
    Server as Server,
    StreamReader as StreamReader,
    StreamReaderProtocol as StreamReaderProtocol,
    StreamWriter as StreamWriter,
    open_connection as open_connection,
    open_unix_connection as open_unix_connection,
    start_server as start_server,
    start_unix_server as start_unix_server,
)
from . import subprocess as subprocess
from .subprocess import (
    Process as Process,
    create_subprocess_exec as create_subprocess_exec,
    create_subprocess_shell as create_subprocess_shell,
)

from . import events as events
from .events import (
    AbstractChildWatcher as AbstractChildWatcher,
    AbstractEventLoop as AbstractEventLoop,
    AbstractEventLoopPolicy as AbstractEventLoopPolicy,
    BaseChildWatcher as BaseChildWatcher,
    BaseEventLoop as BaseEventLoop,
    DefaultEventLoopPolicy as DefaultEventLoopPolicy,
    EventLoop as EventLoop,
    FastChildWatcher as FastChildWatcher,
    Handle as Handle,
    MultiLoopChildWatcher as MultiLoopChildWatcher,
    PidfdChildWatcher as PidfdChildWatcher,
    SafeChildWatcher as SafeChildWatcher,
    SelectorEventLoop as SelectorEventLoop,
    ThreadedChildWatcher as ThreadedChildWatcher,
    TimerHandle as TimerHandle,
    _ProactorEventLoop as _ProactorEventLoop,
    _UnixDefaultEventLoopPolicy as _UnixDefaultEventLoopPolicy,
    _WindowsProactorEventLoopPolicy as _WindowsProactorEventLoopPolicy,
    _WindowsSelectorEventLoopPolicy as _WindowsSelectorEventLoopPolicy,
    _cancel_all_tasks as _cancel_all_tasks,
    _get_running_loop as _get_running_loop,
    _set_running_loop as _set_running_loop,
    can_use_pidfd as can_use_pidfd,
    get_child_watcher as get_child_watcher,
    get_event_loop as get_event_loop,
    get_event_loop_policy as get_event_loop_policy,
    get_running_loop as get_running_loop,
    new_event_loop as new_event_loop,
    on_fork as on_fork,
    set_child_watcher as set_child_watcher,
    set_event_loop as set_event_loop,
    set_event_loop_policy as set_event_loop_policy,
    waitstatus_to_exitcode as waitstatus_to_exitcode,
)
if _EXPOSE_WINDOWS_POLICIES:
    from .events import (
        ProactorEventLoop as ProactorEventLoop,
        WindowsProactorEventLoopPolicy as WindowsProactorEventLoopPolicy,
        WindowsSelectorEventLoopPolicy as WindowsSelectorEventLoopPolicy,
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

log = _make_log_module()
mixins = _make_mixins_module()
locks.exceptions = exceptions
locks.mixins = mixins
queues.locks = locks
queues.mixins = mixins

def __getattr__(name: str) -> Any:
    if name == "log":
        mod = _make_log_module()
    elif name == "mixins":
        mod = _make_mixins_module()
    elif name == "locks":
        mod = locks
    elif name == "queues":
        mod = queues
    else:
        raise AttributeError(f"module 'asyncio' has no attribute '{name}'")
    globals()[name] = mod
    return mod

from . import runners as runners
from . import taskgroups as taskgroups
from . import threads as threads
from . import timeouts as timeouts

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
        "socket": _SOCKET,
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

futures.base_futures = base_futures
futures.events = events
futures.exceptions = exceptions
futures.format_helpers = format_helpers
tasks.base_tasks = base_tasks
tasks.coroutines = coroutines
tasks.events = events
tasks.exceptions = exceptions
tasks.futures = futures
tasks.timeouts = timeouts
setattr(runners, "tasks", tasks)
setattr(timeouts, "tasks", tasks)
setattr(taskgroups, "tasks", tasks)
setattr(subprocess, "tasks", tasks)

streams.coroutines = coroutines
streams.events = events
streams.exceptions = exceptions
streams.format_helpers = format_helpers
streams.protocols = protocols
subprocess.events = events
subprocess.protocols = protocols
subprocess.logger = _logging.getLogger("asyncio")

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

class TransportSocket:
    pass

trsock = _module(
    "asyncio.trsock",
    {
        "TransportSocket": TransportSocket,
        "socket": _SOCKET,
    },
)
setattr(selector_events, "trsock", trsock)

if not _IS_WINDOWS:
    _unix_events_attrs: dict[str, Any] = {
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
        "socket": _SOCKET,
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
        _unix_events_attrs["DefaultEventLoopPolicy"] = _UnixDefaultEventLoopPolicy
    else:
        _unix_all = ["SelectorEventLoop"]
        if _EXPOSE_EVENT_LOOP:
            _unix_all.append("EventLoop")
        _unix_events_attrs["__all__"] = tuple(_unix_all)
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
    "socket": _SOCKET,
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
    if "EventLoop" in globals():
        del globals()["EventLoop"]

if not _EXPOSE_CHILD_WATCHERS:
    for _name in (
        "AbstractChildWatcher",
        "BaseChildWatcher",
        "FastChildWatcher",
        "MultiLoopChildWatcher",
        "PidfdChildWatcher",
        "SafeChildWatcher",
        "ThreadedChildWatcher",
        "get_child_watcher",
        "set_child_watcher",
    ):
        if _name in globals():
            del globals()[_name]

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
        if _name in globals():
            del globals()[_name]

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
    "cast",
    "typing",
):
    globals().pop(_name, None)
