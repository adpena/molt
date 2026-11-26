"""Capability-gated asyncio shim for Molt."""

from __future__ import annotations
from typing import TYPE_CHECKING, Any, Callable, Iterable, cast
import sys as _sys
import types as _types
import typing

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
_IS_WINDOWS = _sys.platform == "win32"
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

    def _molt_async_sleep(_delay: float = 0.0, _result: Any | None = None) -> Any: ...

    def _molt_cancel_token_set_current(_token_id: int) -> int: ...

    def _molt_promise_new() -> Any: ...

    def _molt_promise_set_result(_promise: Any, _value: Any) -> None: ...

    def _molt_cancel_token_get_current() -> int: ...

    def _molt_task_register_execution(_task: Any, _token_id: int, _context: Any) -> None: ...

    def _molt_asyncio_cancel_pending(_tasks: Any) -> int: ...

    def _molt_asyncio_waiters_notify(
        _waiters: Any, _count: int = 1, _result: Any = True
    ) -> int: ...

    def _molt_asyncio_waiters_notify_exception(
        _waiters: Any, _count: int = 1, _exc: Any = None
    ) -> int: ...

    def _molt_asyncio_waiters_remove(_waiters: Any, _waiter: Any) -> bool: ...

    def _molt_asyncio_barrier_release(_waiters: Any) -> int: ...

    def _molt_asyncio_condition_wait_for_step(
        _condition: Any, _predicate: Any
    ) -> tuple[bool, Any]: ...

    def _molt_asyncio_future_transfer(_source: Any, _target: Any) -> bool: ...

    def _molt_asyncio_event_waiters_cleanup(_waiters: Any) -> int: ...

    def _molt_asyncio_task_registry_set(_token_id: int, _task: Any | None) -> None: ...

    def _molt_asyncio_task_registry_get(_token_id: int) -> Any | None: ...

    def _molt_asyncio_task_registry_contains(_token_id: int) -> bool: ...

    def _molt_asyncio_task_registry_current() -> Any | None: ...

    def _molt_asyncio_task_registry_current_for_loop(
        _loop: Any | None = None,
    ) -> Any | None: ...

    def _molt_asyncio_task_registry_pop(_token_id: int) -> Any | None: ...

    def _molt_asyncio_task_registry_move(
        _old_token_id: int, _new_token_id: int
    ) -> bool: ...

    def _molt_asyncio_task_registry_values() -> Any: ...

    def _molt_asyncio_task_registry_live_set(_loop: Any | None = None) -> Any: ...

    def _molt_asyncio_event_waiters_register(_token_id: int, _waiter: Any) -> None: ...

    def _molt_asyncio_event_waiters_unregister(_token_id: int, _waiter: Any) -> bool: ...

    def _molt_asyncio_event_waiters_cleanup_token(_token_id: int) -> int: ...

    def _molt_asyncio_child_watcher_add(
        _callbacks: Any, _pid: int, _callback: Any, _args: tuple[Any, ...]
    ) -> None: ...

    def _molt_asyncio_child_watcher_remove(_callbacks: Any, _pid: int) -> bool: ...

    def _molt_asyncio_child_watcher_clear(_callbacks: Any) -> None: ...

    def _molt_asyncio_child_watcher_pop(
        _callbacks: Any, _pid: int
    ) -> tuple[Any, tuple[Any, ...]] | None: ...

    def _molt_asyncio_ssl_transport_orchestrate(
        _operation: str,
        _ssl: Any,
        _server_hostname: str | None = None,
        _server_side: bool = False,
    ) -> bool: ...

    def _molt_asyncio_tls_client_connect_new(
        _host: str, _port: int, _server_hostname: str | None = None
    ) -> Any: ...

    def _molt_asyncio_tls_client_from_fd_new(
        _fd: int, _server_hostname: str | None = None
    ) -> Any: ...

    def _molt_asyncio_tls_server_payload(_ssl: Any) -> tuple[str, str]: ...

    def _molt_asyncio_tls_server_from_fd_new(
        _fd: int, _certfile: str, _keyfile: str
    ) -> Any: ...

    def _molt_asyncio_require_unix_socket_support() -> None: ...

    def _molt_asyncio_require_child_watcher_support() -> None: ...

    def _molt_asyncio_running_loop_get() -> Any: ...

    def _molt_asyncio_running_loop_set(_loop: Any) -> None: ...

    def _molt_asyncio_event_loop_get_current() -> Any: ...

    def _molt_asyncio_event_loop_set(_loop: Any) -> None: ...

    def _molt_asyncio_event_loop_policy_get() -> Any: ...

    def _molt_asyncio_event_loop_policy_set(_policy: Any) -> None: ...

    def _molt_asyncio_tasks_add_done_callback(_tasks: Any, _callback: Any) -> int: ...

    def _molt_asyncio_task_cancel_apply(
        _future: Any, _msg: Any | None = None
    ) -> bool: ...

    def _molt_asyncio_task_uncancel_apply(_future: Any) -> None: ...

    def _molt_asyncio_event_set_waiters(_waiters: Any, _result: Any = True) -> int: ...

    def _molt_asyncio_fd_watcher_register(
        _loop: Any,
        _registry: Any,
        _fileno: Any,
        _callback: Any,
        _args: Any,
        _events: Any,
    ) -> None: ...

    def _molt_asyncio_fd_watcher_unregister(_registry: Any, _fileno: Any) -> bool: ...

    def _molt_pipe_transport_new(_fd: Any, _is_read: Any) -> Any: ...

    def _molt_pipe_transport_get_fd(_handle: Any) -> int: ...

    def _molt_pipe_transport_is_closing(_handle: Any) -> bool: ...

    def _molt_pipe_transport_close(_handle: Any) -> None: ...

    def _molt_pipe_transport_pause_reading(_handle: Any) -> None: ...

    def _molt_pipe_transport_resume_reading(_handle: Any) -> None: ...

    def _molt_pipe_transport_write(_handle: Any, _data: Any) -> None: ...

    def _molt_pipe_transport_get_write_buffer_size(_handle: Any) -> int: ...

    def _molt_pipe_transport_drop(_handle: Any) -> None: ...

    def _molt_asyncio_subprocess_stdio_normalize(
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

    def _molt_asyncio_server_accept_loop_new(
        _sock: Any,
        _callback: Any,
        _loop: Any,
        _reader_ctor: Any,
        _writer_ctor: Any,
        _closed_probe: Any,
    ) -> Any: ...

    def _molt_asyncio_stream_reader_read_new(_reader: Any, _n: int = -1) -> Any: ...

    def _molt_asyncio_stream_reader_readline_new(_reader: Any) -> Any: ...

    def _molt_asyncio_stream_send_all_new(_stream: Any, _data: Any) -> Any: ...

    def _molt_asyncio_stream_buffer_snapshot(_buffer: Any) -> Any: ...

    def _molt_asyncio_stream_buffer_consume(_buffer: Any, _count: int) -> int: ...

    def _molt_asyncio_socket_reader_read_new(
        _reader: Any, _n: int = -1, _fd: int = -1
    ) -> Any: ...

    def _molt_asyncio_socket_reader_readline_new(_reader: Any, _fd: int = -1) -> Any: ...

    def _molt_asyncio_sock_recv_new(_sock: Any, _size: int, _fd: int) -> Any: ...

    def _molt_asyncio_sock_connect_new(_sock: Any, _address: Any, _fd: int) -> Any: ...

    def _molt_asyncio_sock_accept_new(_sock: Any, _fd: int) -> Any: ...

    def _molt_asyncio_sock_recv_into_new(
        _sock: Any, _buf: Any, _nbytes: int, _fd: int
    ) -> Any: ...

    def _molt_asyncio_sock_sendall_new(_sock: Any, _data: Any, _fd: int) -> Any: ...

    def _molt_asyncio_sock_recvfrom_new(_sock: Any, _size: int, _fd: int) -> Any: ...

    def _molt_asyncio_sock_recvfrom_into_new(
        _sock: Any, _buf: Any, _nbytes: int, _fd: int
    ) -> Any: ...

    def _molt_asyncio_sock_sendto_new(
        _sock: Any, _data: Any, _addr: Any, _fd: int
    ) -> Any: ...

    # Handle-based state machine intrinsics -- Future
    def _molt_asyncio_future_new() -> int: ...

    def _molt_asyncio_future_result(_handle: int) -> Any: ...

    def _molt_asyncio_future_exception(_handle: int) -> Any: ...

    def _molt_asyncio_future_set_result_fast(_handle: int, _result: Any) -> None: ...

    def _molt_asyncio_future_set_exception_fast(_handle: int, _exc: Any) -> None: ...

    def _molt_asyncio_future_cancel_fast(_handle: int, _msg: Any) -> bool: ...

    def _molt_asyncio_future_done(_handle: int) -> bool: ...

    def _molt_asyncio_future_cancelled(_handle: int) -> bool: ...

    def _molt_asyncio_future_drop(_handle: int) -> None: ...

    # Handle-based state machine intrinsics -- Event
    def _molt_asyncio_event_new() -> int: ...

    def _molt_asyncio_event_is_set(_handle: int) -> bool: ...

    def _molt_asyncio_event_set_fast(_handle: int) -> int: ...

    def _molt_asyncio_event_clear_handle(_handle: int) -> None: ...

    def _molt_asyncio_event_drop(_handle: int) -> None: ...

    # Handle-based state machine intrinsics -- Lock
    def _molt_asyncio_lock_new() -> int: ...

    def _molt_asyncio_lock_locked(_handle: int) -> bool: ...

    def _molt_asyncio_lock_acquire_fast(_handle: int) -> bool: ...

    def _molt_asyncio_lock_release_fast(_handle: int) -> int: ...

    def _molt_asyncio_lock_drop(_handle: int) -> None: ...

    # Handle-based state machine intrinsics -- Semaphore
    def _molt_asyncio_semaphore_new(_value: int) -> int: ...

    def _molt_asyncio_semaphore_acquire_fast(_handle: int) -> bool: ...

    def _molt_asyncio_semaphore_release_fast(_handle: int, _max_value: int) -> int: ...

    def _molt_asyncio_semaphore_value(_handle: int) -> int: ...

    def _molt_asyncio_semaphore_drop(_handle: int) -> None: ...

    # Handle-based state machine intrinsics -- Queue
    def _molt_asyncio_queue_new(_maxsize: int, _queue_type: int) -> int: ...

    def _molt_asyncio_queue_put_nowait(_handle: int, _item: Any) -> None: ...

    def _molt_asyncio_queue_get_nowait(_handle: int) -> Any: ...

    def _molt_asyncio_queue_qsize(_handle: int) -> int: ...

    def _molt_asyncio_queue_maxsize(_handle: int) -> int: ...

    def _molt_asyncio_queue_empty(_handle: int) -> bool: ...

    def _molt_asyncio_queue_full(_handle: int) -> bool: ...

    def _molt_asyncio_queue_task_done(_handle: int) -> None: ...

    def _molt_asyncio_queue_unfinished_tasks(_handle: int) -> int: ...

    def _molt_asyncio_queue_shutdown(_handle: int, _immediate: bool) -> None: ...

    def _molt_asyncio_queue_is_shutdown(_handle: int) -> bool: ...

    def _molt_asyncio_queue_drop(_handle: int) -> None: ...

    # Handle-based event loop intrinsics (RT2 core)
    def _molt_event_loop_spawn(_loop_handle: Any, _runner: Any) -> None: ...

    def _molt_event_loop_new() -> Any: ...

    def _molt_event_loop_call_soon(_loop_handle: Any, _callback: Any) -> None: ...

    def _molt_event_loop_call_at(
        _loop_handle: Any, _when: Any, _callback: Any
    ) -> Any: ...

    def _molt_event_loop_cancel_timer(_loop_handle: Any, _timer_id: Any) -> None: ...

    def _molt_event_loop_add_reader(
        _loop_handle: Any, _fd: Any, _callback: Any
    ) -> None: ...

    def _molt_event_loop_remove_reader(_loop_handle: Any, _fd: Any) -> bool: ...

    def _molt_event_loop_add_writer(
        _loop_handle: Any, _fd: Any, _callback: Any
    ) -> None: ...

    def _molt_event_loop_remove_writer(_loop_handle: Any, _fd: Any) -> bool: ...

    def _molt_event_loop_run_once(_loop_handle: Any) -> int: ...

    def _molt_event_loop_time(_loop_handle: Any) -> float: ...

    def _molt_event_loop_wait(_loop_handle: Any) -> None: ...

    def _molt_event_loop_wake(_loop_handle: Any) -> None: ...

    def _molt_event_loop_start(_loop_handle: Any) -> None: ...

    def _molt_event_loop_stop(_loop_handle: Any) -> None: ...

    def _molt_event_loop_is_running(_loop_handle: Any) -> bool: ...

    def _molt_event_loop_is_closed(_loop_handle: Any) -> bool: ...

    def _molt_event_loop_close(_loop_handle: Any) -> None: ...

    def _molt_event_loop_drop(_loop_handle: Any) -> None: ...

    def _molt_event_loop_set_debug(_loop_handle: Any, _enabled: Any) -> None: ...

    def _molt_event_loop_get_debug(_loop_handle: Any) -> bool: ...

    def _molt_event_loop_set_exception_handler(
        _loop_handle: Any, _handler: Any
    ) -> None: ...

    def _molt_event_loop_get_exception_handler(_loop_handle: Any) -> Any: ...

    def _molt_event_loop_set_task_factory(_loop_handle: Any, _factory: Any) -> None: ...

    def _molt_event_loop_get_task_factory(_loop_handle: Any) -> Any: ...

    def _molt_event_loop_notify_reader_ready(_loop_handle: Any, _fd: Any) -> None: ...

    def _molt_event_loop_notify_writer_ready(_loop_handle: Any, _fd: Any) -> None: ...

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
    return isinstance(exc, CancelledError)

from .coroutines import (
    iscoroutine as iscoroutine,
    iscoroutinefunction as iscoroutinefunction,
)

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
        _molt_asyncio_ssl_transport_orchestrate,
        "asyncio_ssl_transport_orchestrate",
    )(operation, ssl, server_hostname, server_side)
    if not isinstance(outcome, bool):
        raise RuntimeError("asyncio ssl transport intrinsic returned invalid payload")
    return outcome

def _tls_client_connect(
    host: str, port: int, server_hostname: str | None = None
) -> Any:
    return _require_asyncio_intrinsic(
        _molt_asyncio_tls_client_connect_new,
        "asyncio_tls_client_connect_new",
    )(host, port, server_hostname)

def _tls_client_from_fd(fd: int, server_hostname: str | None = None) -> Any:
    return _require_asyncio_intrinsic(
        _molt_asyncio_tls_client_from_fd_new,
        "asyncio_tls_client_from_fd_new",
    )(fd, server_hostname)

def _tls_server_payload(ssl: Any) -> tuple[str, str]:
    payload = _require_asyncio_intrinsic(
        _molt_asyncio_tls_server_payload, "asyncio_tls_server_payload"
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
        _molt_asyncio_tls_server_from_fd_new,
        "asyncio_tls_server_from_fd_new",
    )(fd, certfile, keyfile)

def _require_unix_socket_support() -> None:
    _require_asyncio_intrinsic(
        _molt_asyncio_require_unix_socket_support,
        "asyncio_require_unix_socket_support",
    )()

def _require_child_watcher_support() -> None:
    _require_asyncio_intrinsic(
        _molt_asyncio_require_child_watcher_support,
        "asyncio_require_child_watcher_support",
    )()

def _asyncio_cancel_pending_tasks(tasks: Any) -> int:
    return int(
        _require_asyncio_intrinsic(
            _molt_asyncio_cancel_pending, "asyncio_cancel_pending"
        )(tasks)
    )

def _asyncio_waiters_notify(waiters: Any, count: int, result: Any) -> int:
    return int(
        _require_asyncio_intrinsic(
            _molt_asyncio_waiters_notify, "asyncio_waiters_notify"
        )(waiters, count, result)
    )

def _asyncio_waiters_notify_exception(waiters: Any, count: int, exc: Any) -> int:
    return int(
        _require_asyncio_intrinsic(
            _molt_asyncio_waiters_notify_exception, "asyncio_waiters_notify_exception"
        )(waiters, count, exc)
    )

def _asyncio_waiters_remove(waiters: Any, waiter: Any) -> bool:
    return bool(
        _require_asyncio_intrinsic(
            _molt_asyncio_waiters_remove, "asyncio_waiters_remove"
        )(waiters, waiter)
    )

def _asyncio_barrier_release(waiters: Any) -> int:
    return int(
        _require_asyncio_intrinsic(
            _molt_asyncio_barrier_release, "asyncio_barrier_release"
        )(waiters)
    )

def _asyncio_condition_wait_for_step(
    condition: Any, predicate: Callable[[], Any]
) -> tuple[bool, Any]:
    done, payload = _require_asyncio_intrinsic(
        _molt_asyncio_condition_wait_for_step, "asyncio_condition_wait_for_step"
    )(condition, predicate)
    return bool(done), payload

def _asyncio_future_transfer(source: Any, target: Any) -> bool:
    return bool(
        _require_asyncio_intrinsic(
            _molt_asyncio_future_transfer, "asyncio_future_transfer"
        )(source, target)
    )

def _asyncio_event_waiters_cleanup(waiters: Any) -> int:
    return int(
        _require_asyncio_intrinsic(
            _molt_asyncio_event_waiters_cleanup, "asyncio_event_waiters_cleanup"
        )(waiters)
    )

def _task_registry_set(token_id: int, task: Any | None) -> None:
    _require_asyncio_intrinsic(
        _molt_asyncio_task_registry_set, "asyncio_task_registry_set"
    )(token_id, task)

def _task_registry_get(token_id: int) -> Any | None:
    return _require_asyncio_intrinsic(
        _molt_asyncio_task_registry_get, "asyncio_task_registry_get"
    )(token_id)

def _task_registry_contains(token_id: int) -> bool:
    return bool(
        _require_asyncio_intrinsic(
            _molt_asyncio_task_registry_contains, "asyncio_task_registry_contains"
        )(token_id)
    )

def _task_registry_current() -> Any | None:
    return _require_asyncio_intrinsic(
        _molt_asyncio_task_registry_current, "asyncio_task_registry_current"
    )()

def _task_registry_current_for_loop(loop: Any | None = None) -> Any | None:
    return _require_asyncio_intrinsic(
        _molt_asyncio_task_registry_current_for_loop,
        "asyncio_task_registry_current_for_loop",
    )(loop)

def _task_registry_pop(token_id: int) -> Any | None:
    return _require_asyncio_intrinsic(
        _molt_asyncio_task_registry_pop, "asyncio_task_registry_pop"
    )(token_id)

def _task_registry_move(old_token_id: int, new_token_id: int) -> bool:
    return bool(
        _require_asyncio_intrinsic(
            _molt_asyncio_task_registry_move, "asyncio_task_registry_move"
        )(old_token_id, new_token_id)
    )

def _task_registry_values() -> Any:
    return _require_asyncio_intrinsic(
        _molt_asyncio_task_registry_values, "asyncio_task_registry_values"
    )()

def _event_waiters_register(token_id: int, waiter: Any) -> None:
    _require_asyncio_intrinsic(
        _molt_asyncio_event_waiters_register, "asyncio_event_waiters_register"
    )(token_id, waiter)

def _event_waiters_unregister(token_id: int, waiter: Any) -> bool:
    return bool(
        _require_asyncio_intrinsic(
            _molt_asyncio_event_waiters_unregister, "asyncio_event_waiters_unregister"
        )(token_id, waiter)
    )

def _event_waiters_cleanup_token(token_id: int) -> int:
    return int(
        _require_asyncio_intrinsic(
            _molt_asyncio_event_waiters_cleanup_token,
            "asyncio_event_waiters_cleanup_token",
        )(token_id)
    )


def _asyncio_tasks_add_done_callback(tasks: Any, callback: Callable[[Any], Any]) -> int:
    return int(
        _require_asyncio_intrinsic(
            _molt_asyncio_tasks_add_done_callback, "asyncio_tasks_add_done_callback"
        )(tasks, callback)
    )

async def _async_yield_once() -> None:
    fut = _molt_async_sleep(0.0, None)
    await fut

async def _io_wait(fd: int, events: int, timeout: float | None = None) -> Any:
    io_wait = _require_io_wait_new()
    return await _require_asyncio_intrinsic(io_wait, "io_wait_new")(fd, events, timeout)

_molt_io_wait_new = _intrinsic_require("molt_io_wait_new")
_molt_pending = _intrinsic_require("molt_pending")
_molt_async_sleep = _intrinsic_require("molt_async_sleep")
_molt_spawn = _intrinsic_require("molt_spawn")
_molt_cancel_token_new = _intrinsic_require("molt_cancel_token_new")
_molt_cancel_token_clone = _intrinsic_require("molt_cancel_token_clone")
_molt_cancel_token_drop = _intrinsic_require("molt_cancel_token_drop")
_molt_cancel_token_cancel = _intrinsic_require("molt_cancel_token_cancel")
_molt_cancel_token_is_cancelled = _intrinsic_require(
    "molt_cancel_token_is_cancelled"
)
_molt_cancel_token_set_current = _intrinsic_require(
    "molt_cancel_token_set_current"
)
_molt_cancel_token_get_current = _intrinsic_require(
    "molt_cancel_token_get_current"
)
_molt_promise_new = _intrinsic_require("molt_promise_new")
_molt_promise_set_result = _intrinsic_require("molt_promise_set_result")
_molt_task_register_execution = _intrinsic_require(
    "molt_task_register_execution"
)
_molt_asyncio_cancel_pending = _intrinsic_require(
    "molt_asyncio_cancel_pending"
)
_molt_asyncio_waiters_notify = _intrinsic_require(
    "molt_asyncio_waiters_notify"
)
_molt_asyncio_waiters_notify_exception = _intrinsic_require(
    "molt_asyncio_waiters_notify_exception"
)
_molt_asyncio_waiters_remove = _intrinsic_require(
    "molt_asyncio_waiters_remove"
)
_molt_asyncio_barrier_release = _intrinsic_require(
    "molt_asyncio_barrier_release"
)
_molt_asyncio_condition_wait_for_step = _intrinsic_require(
    "molt_asyncio_condition_wait_for_step"
)
_molt_asyncio_future_transfer = _intrinsic_require(
    "molt_asyncio_future_transfer"
)
_molt_asyncio_event_waiters_cleanup = _intrinsic_require(
    "molt_asyncio_event_waiters_cleanup"
)
_molt_asyncio_task_registry_set = _intrinsic_require(
    "molt_asyncio_task_registry_set"
)
_molt_asyncio_task_registry_get = _intrinsic_require(
    "molt_asyncio_task_registry_get"
)
_molt_asyncio_task_registry_contains = _intrinsic_require(
    "molt_asyncio_task_registry_contains"
)
_molt_asyncio_task_registry_current = _intrinsic_require(
    "molt_asyncio_task_registry_current"
)
_molt_asyncio_task_registry_current_for_loop = _intrinsic_require(
    "molt_asyncio_task_registry_current_for_loop"
)
_molt_asyncio_task_registry_pop = _intrinsic_require(
    "molt_asyncio_task_registry_pop"
)
_molt_asyncio_task_last_exception_clear = _intrinsic_require(
    "molt_asyncio_task_last_exception_clear"
)
_molt_asyncio_task_registry_move = _intrinsic_require(
    "molt_asyncio_task_registry_move"
)
_molt_asyncio_task_registry_values = _intrinsic_require(
    "molt_asyncio_task_registry_values"
)
_molt_asyncio_task_registry_live_set = _intrinsic_require(
    "molt_asyncio_task_registry_live_set"
)
_molt_asyncio_event_waiters_register = _intrinsic_require(
    "molt_asyncio_event_waiters_register"
)
_molt_asyncio_event_waiters_unregister = _intrinsic_require(
    "molt_asyncio_event_waiters_unregister"
)
_molt_asyncio_event_waiters_cleanup_token = _intrinsic_require(
    "molt_asyncio_event_waiters_cleanup_token"
)
_molt_asyncio_child_watcher_add = _intrinsic_require(
    "molt_asyncio_child_watcher_add"
)
_molt_asyncio_child_watcher_remove = _intrinsic_require(
    "molt_asyncio_child_watcher_remove"
)
_molt_asyncio_child_watcher_clear = _intrinsic_require(
    "molt_asyncio_child_watcher_clear"
)
_molt_asyncio_child_watcher_pop = _intrinsic_require(
    "molt_asyncio_child_watcher_pop"
)
_molt_asyncio_ssl_transport_orchestrate = _intrinsic_require(
    "molt_asyncio_ssl_transport_orchestrate"
)
_molt_asyncio_tls_client_connect_new = _intrinsic_require(
    "molt_asyncio_tls_client_connect_new"
)
_molt_asyncio_tls_client_from_fd_new = _intrinsic_require(
    "molt_asyncio_tls_client_from_fd_new"
)
_molt_asyncio_tls_server_payload = _intrinsic_require(
    "molt_asyncio_tls_server_payload"
)
_molt_asyncio_tls_server_from_fd_new = _intrinsic_require(
    "molt_asyncio_tls_server_from_fd_new"
)
_molt_asyncio_require_unix_socket_support = _intrinsic_require(
    "molt_asyncio_require_unix_socket_support"
)
_molt_asyncio_require_child_watcher_support = _intrinsic_require(
    "molt_asyncio_require_child_watcher_support"
)
_molt_asyncio_running_loop_get = _intrinsic_require(
    "molt_asyncio_running_loop_get"
)
_molt_asyncio_running_loop_set = _intrinsic_require(
    "molt_asyncio_running_loop_set"
)
_molt_asyncio_event_loop_get_current = _intrinsic_require(
    "molt_asyncio_event_loop_get_current"
)
_molt_asyncio_event_loop_set = _intrinsic_require(
    "molt_asyncio_event_loop_set"
)
_molt_asyncio_event_loop_policy_get = _intrinsic_require(
    "molt_asyncio_event_loop_policy_get"
)
_molt_asyncio_event_loop_policy_set = _intrinsic_require(
    "molt_asyncio_event_loop_policy_set"
)
_molt_asyncio_tasks_add_done_callback = _intrinsic_require(
    "molt_asyncio_tasks_add_done_callback"
)
_molt_asyncio_task_cancel_apply = _intrinsic_require(
    "molt_asyncio_task_cancel_apply"
)
_molt_asyncio_task_uncancel_apply = _intrinsic_require(
    "molt_asyncio_task_uncancel_apply"
)
_molt_asyncio_event_set_waiters = _intrinsic_require(
    "molt_asyncio_event_set_waiters"
)
_molt_asyncio_fd_watcher_register = _intrinsic_require(
    "molt_asyncio_fd_watcher_register"
)
_molt_asyncio_fd_watcher_unregister = _intrinsic_require(
    "molt_asyncio_fd_watcher_unregister"
)
# --- Event loop Rust handle intrinsics (RT2 core, 28 total) ---
_molt_event_loop_spawn = _intrinsic_require("molt_event_loop_spawn")
_molt_event_loop_new = _intrinsic_require("molt_event_loop_new")
_molt_event_loop_call_soon = _intrinsic_require("molt_event_loop_call_soon")
_molt_event_loop_call_at = _intrinsic_require("molt_event_loop_call_at")
_molt_event_loop_cancel_timer = _intrinsic_require(
    "molt_event_loop_cancel_timer"
)
_molt_event_loop_add_reader = _intrinsic_require("molt_event_loop_add_reader")
_molt_event_loop_remove_reader = _intrinsic_require(
    "molt_event_loop_remove_reader"
)
_molt_event_loop_add_writer = _intrinsic_require("molt_event_loop_add_writer")
_molt_event_loop_remove_writer = _intrinsic_require(
    "molt_event_loop_remove_writer"
)
_molt_event_loop_run_once = _intrinsic_require("molt_event_loop_run_once")
_molt_event_loop_time = _intrinsic_require("molt_event_loop_time")
_molt_event_loop_wait = _intrinsic_require("molt_event_loop_wait")
_molt_event_loop_wake = _intrinsic_require("molt_event_loop_wake")
_molt_event_loop_start = _intrinsic_require("molt_event_loop_start")
_molt_event_loop_stop = _intrinsic_require("molt_event_loop_stop")
_molt_event_loop_is_running = _intrinsic_require("molt_event_loop_is_running")
_molt_event_loop_is_closed = _intrinsic_require("molt_event_loop_is_closed")
_molt_event_loop_close = _intrinsic_require("molt_event_loop_close")
_molt_event_loop_drop = _intrinsic_require("molt_event_loop_drop")
_molt_event_loop_set_debug = _intrinsic_require("molt_event_loop_set_debug")
_molt_event_loop_get_debug = _intrinsic_require("molt_event_loop_get_debug")
_molt_event_loop_set_exception_handler = _intrinsic_require(
    "molt_event_loop_set_exception_handler"
)
_molt_event_loop_get_exception_handler = _intrinsic_require(
    "molt_event_loop_get_exception_handler"
)
_molt_event_loop_set_task_factory = _intrinsic_require(
    "molt_event_loop_set_task_factory"
)
_molt_event_loop_get_task_factory = _intrinsic_require(
    "molt_event_loop_get_task_factory"
)
_molt_event_loop_notify_reader_ready = _intrinsic_require(
    "molt_event_loop_notify_reader_ready"
)
_molt_event_loop_notify_writer_ready = _intrinsic_require(
    "molt_event_loop_notify_writer_ready"
)
_molt_pipe_transport_new = _intrinsic_require("molt_pipe_transport_new")
_molt_pipe_transport_get_fd = _intrinsic_require("molt_pipe_transport_get_fd")
_molt_pipe_transport_is_closing = _intrinsic_require(
    "molt_pipe_transport_is_closing"
)
_molt_pipe_transport_close = _intrinsic_require("molt_pipe_transport_close")
_molt_pipe_transport_pause_reading = _intrinsic_require(
    "molt_pipe_transport_pause_reading"
)
_molt_pipe_transport_resume_reading = _intrinsic_require(
    "molt_pipe_transport_resume_reading"
)
_molt_pipe_transport_write = _intrinsic_require("molt_pipe_transport_write")
_molt_pipe_transport_get_write_buffer_size = _intrinsic_require(
    "molt_pipe_transport_get_write_buffer_size"
)
_molt_pipe_transport_drop = _intrinsic_require("molt_pipe_transport_drop")
_molt_asyncio_subprocess_stdio_normalize = _intrinsic_require(
    "molt_asyncio_subprocess_stdio_normalize"
)
_molt_asyncio_server_accept_loop_new = _intrinsic_require(
    "molt_asyncio_server_accept_loop_new"
)
_molt_asyncio_stream_reader_read_new = _intrinsic_require(
    "molt_asyncio_stream_reader_read_new"
)
_molt_asyncio_stream_reader_readline_new = _intrinsic_require(
    "molt_asyncio_stream_reader_readline_new"
)
_molt_asyncio_stream_send_all_new = _intrinsic_require(
    "molt_asyncio_stream_send_all_new"
)
_molt_asyncio_stream_buffer_snapshot = _intrinsic_require(
    "molt_asyncio_stream_buffer_snapshot"
)
_molt_asyncio_stream_buffer_consume = _intrinsic_require(
    "molt_asyncio_stream_buffer_consume"
)
_molt_asyncio_socket_reader_read_new = _intrinsic_require(
    "molt_asyncio_socket_reader_read_new"
)
_molt_asyncio_socket_reader_readline_new = _intrinsic_require(
    "molt_asyncio_socket_reader_readline_new"
)
_molt_asyncio_sock_recv_new = _intrinsic_require("molt_asyncio_sock_recv_new")
_molt_asyncio_sock_connect_new = _intrinsic_require(
    "molt_asyncio_sock_connect_new"
)
_molt_asyncio_sock_accept_new = _intrinsic_require(
    "molt_asyncio_sock_accept_new"
)
_molt_asyncio_sock_recv_into_new = _intrinsic_require(
    "molt_asyncio_sock_recv_into_new"
)
_molt_asyncio_sock_sendall_new = _intrinsic_require(
    "molt_asyncio_sock_sendall_new"
)
_molt_asyncio_sock_recvfrom_new = _intrinsic_require(
    "molt_asyncio_sock_recvfrom_new"
)
_molt_asyncio_sock_recvfrom_into_new = _intrinsic_require(
    "molt_asyncio_sock_recvfrom_into_new"
)
_molt_asyncio_sock_sendto_new = _intrinsic_require(
    "molt_asyncio_sock_sendto_new"
)
_molt_generic_alias_new = _intrinsic_require("molt_generic_alias_new")

_molt_function_set_builtin = _intrinsic_require("molt_function_set_builtin")
_molt_process_spawn = _intrinsic_require("molt_process_spawn")
_molt_process_wait_future = _intrinsic_require("molt_process_wait_future")
_molt_process_pid = _intrinsic_require("molt_process_pid")
_molt_process_returncode = _intrinsic_require("molt_process_returncode")
_molt_process_kill = _intrinsic_require("molt_process_kill")
_molt_process_terminate = _intrinsic_require("molt_process_terminate")
_molt_process_stdin = _intrinsic_require("molt_process_stdin")
_molt_process_stdout = _intrinsic_require("molt_process_stdout")
_molt_process_stderr = _intrinsic_require("molt_process_stderr")
_molt_process_drop = _intrinsic_require("molt_process_drop")
_molt_stream_close = _intrinsic_require("molt_stream_close")
_molt_stream_reader_new = _intrinsic_require("molt_stream_reader_new")
_molt_stream_reader_at_eof = _intrinsic_require("molt_stream_reader_at_eof")
_molt_stream_reader_drop = _intrinsic_require("molt_stream_reader_drop")
_molt_socket_reader_new = _intrinsic_require("molt_socket_reader_new")
_molt_socket_reader_at_eof = _intrinsic_require("molt_socket_reader_at_eof")
_molt_socket_reader_drop = _intrinsic_require("molt_socket_reader_drop")

# Handle-based state machine intrinsics -- Future
_molt_asyncio_future_new = _intrinsic_require("molt_asyncio_future_new")
_molt_asyncio_future_result = _intrinsic_require("molt_asyncio_future_result")
_molt_asyncio_future_exception = _intrinsic_require(
    "molt_asyncio_future_exception"
)
_molt_asyncio_future_set_result_fast = _intrinsic_require(
    "molt_asyncio_future_set_result_fast"
)
_molt_asyncio_future_set_exception_fast = _intrinsic_require(
    "molt_asyncio_future_set_exception_fast"
)
_molt_asyncio_future_cancel_fast = _intrinsic_require(
    "molt_asyncio_future_cancel_fast"
)
_molt_asyncio_future_done = _intrinsic_require("molt_asyncio_future_done")
_molt_asyncio_future_cancelled = _intrinsic_require(
    "molt_asyncio_future_cancelled"
)
_molt_asyncio_future_drop = _intrinsic_require("molt_asyncio_future_drop")

# Handle-based state machine intrinsics -- Event
_molt_asyncio_event_new = _intrinsic_require("molt_asyncio_event_new")
_molt_asyncio_event_is_set = _intrinsic_require("molt_asyncio_event_is_set")
_molt_asyncio_event_set_fast = _intrinsic_require(
    "molt_asyncio_event_set_fast"
)
_molt_asyncio_event_clear_handle = _intrinsic_require(
    "molt_asyncio_event_clear"
)
_molt_asyncio_event_drop = _intrinsic_require("molt_asyncio_event_drop")

# Handle-based state machine intrinsics -- Lock
_molt_asyncio_lock_new = _intrinsic_require("molt_asyncio_lock_new")
_molt_asyncio_lock_locked = _intrinsic_require("molt_asyncio_lock_locked")
_molt_asyncio_lock_acquire_fast = _intrinsic_require(
    "molt_asyncio_lock_acquire_fast"
)
_molt_asyncio_lock_release_fast = _intrinsic_require(
    "molt_asyncio_lock_release_fast"
)
_molt_asyncio_lock_drop = _intrinsic_require("molt_asyncio_lock_drop")

# Handle-based state machine intrinsics -- Semaphore
_molt_asyncio_semaphore_new = _intrinsic_require("molt_asyncio_semaphore_new")
_molt_asyncio_semaphore_acquire_fast = _intrinsic_require(
    "molt_asyncio_semaphore_acquire_fast"
)
_molt_asyncio_semaphore_release_fast = _intrinsic_require(
    "molt_asyncio_semaphore_release_fast"
)
_molt_asyncio_semaphore_value = _intrinsic_require(
    "molt_asyncio_semaphore_value"
)
_molt_asyncio_semaphore_drop = _intrinsic_require(
    "molt_asyncio_semaphore_drop"
)

# Handle-based state machine intrinsics -- Queue
_molt_asyncio_queue_new = _intrinsic_require("molt_asyncio_queue_new")
_molt_asyncio_queue_put_nowait = _intrinsic_require(
    "molt_asyncio_queue_put_nowait"
)
_molt_asyncio_queue_get_nowait = _intrinsic_require(
    "molt_asyncio_queue_get_nowait"
)
_molt_asyncio_queue_qsize = _intrinsic_require("molt_asyncio_queue_qsize")
_molt_asyncio_queue_maxsize = _intrinsic_require("molt_asyncio_queue_maxsize")
_molt_asyncio_queue_empty = _intrinsic_require("molt_asyncio_queue_empty")
_molt_asyncio_queue_full = _intrinsic_require("molt_asyncio_queue_full")
_molt_asyncio_queue_task_done = _intrinsic_require(
    "molt_asyncio_queue_task_done"
)
_molt_asyncio_queue_unfinished_tasks = _intrinsic_require(
    "molt_asyncio_queue_unfinished_tasks"
)
_molt_asyncio_queue_shutdown = _intrinsic_require(
    "molt_asyncio_queue_shutdown"
)
_molt_asyncio_queue_is_shutdown = _intrinsic_require(
    "molt_asyncio_queue_is_shutdown"
)
_molt_asyncio_queue_drop = _intrinsic_require("molt_asyncio_queue_drop")

_PENDING_SENTINEL: Any | None = None

def _pending_sentinel() -> Any:
    global _PENDING_SENTINEL
    if _PENDING_SENTINEL is None:
        _PENDING_SENTINEL = _molt_pending()
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
            _molt_asyncio_subprocess_stdio_normalize,
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
    create_eager_task_factory as create_eager_task_factory,
    create_task as create_task,
    current_task as current_task,
    eager_task_factory as eager_task_factory,
    ensure_future as ensure_future,
    gather as gather,
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

if _EXPOSE_GRAPH:
    from .tasks import (
        FrameCallGraphEntry as FrameCallGraphEntry,
        FutureCallGraph as FutureCallGraph,
        capture_call_graph as capture_call_graph,
        format_call_graph as format_call_graph,
        print_call_graph as print_call_graph,
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

from . import constants as constants
from . import coroutines as coroutines
from . import format_helpers as format_helpers
from . import log as log
from . import mixins as mixins

setattr(events, "format_helpers", format_helpers)
locks.exceptions = exceptions
locks.mixins = mixins
queues.locks = locks
queues.mixins = mixins

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

from . import base_futures as base_futures
from . import base_tasks as base_tasks
from . import base_subprocess as base_subprocess

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

streams.coroutines = coroutines
streams.events = events
streams.exceptions = exceptions
streams.format_helpers = format_helpers
streams.protocols = protocols
subprocess.events = events

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

from . import staggered as staggered
from . import sslproto as sslproto
from . import trsock as trsock
from . import base_events as base_events
from . import selector_events as selector_events

if _IS_WINDOWS:
    from . import windows_events as windows_events
    from . import windows_utils as windows_utils
else:
    from . import unix_events as unix_events

if _EXPOSE_GRAPH:
    from . import graph as graph
    from . import tools as tools

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
