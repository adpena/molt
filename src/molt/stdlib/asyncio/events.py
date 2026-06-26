"""Event-loop, policy, watcher, and transport-shim authority for ``asyncio.events``."""

from __future__ import annotations

import contextvars
import os
import signal
import subprocess
import sys
import threading
import time as _time
import warnings as _warnings
from collections import deque as _deque
from typing import TYPE_CHECKING, Any, Callable, cast as _cast

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

import asyncio as _asyncio
from asyncio import (
    Future,
    ProcessStreamWriter,
    _RUN_FOREVER_IDLE_CAP,
    StreamReader,
    StreamWriter,
    Task,
    _DEBUG_ASYNCIO_EXC,
    _DEBUG_ASYNCIO_HANDLES,
    _DEBUG_ASYNCIO_SHUTDOWN,
    _EXPOSE_CHILD_WATCHERS,
    _EXPOSE_WINDOWS_POLICIES,
    _IS_WINDOWS,
    _asyncio_cancel_pending_tasks,
    _debug_exc_state,
    _debug_task_summary,
    _debug_write,
    _fd_from_fileobj,
    _restore_token_id,
    _require_asyncio_intrinsic,
    _require_child_watcher_support,
    _require_ssl_transport_support,
    _socket_wait_key,
    _swap_current_token,
    _socket_module,
    _tls_client_from_fd,
    _tls_server_from_fd,
    _tls_server_payload,
    all_tasks,
    create_subprocess_exec,
    create_subprocess_shell,
    molt_asyncio_child_watcher_add,
    molt_asyncio_child_watcher_clear,
    molt_asyncio_child_watcher_pop,
    molt_asyncio_child_watcher_remove,
    molt_asyncio_event_loop_get_current,
    molt_asyncio_event_loop_policy_get,
    molt_asyncio_event_loop_policy_set,
    molt_asyncio_event_loop_set,
    molt_asyncio_fd_watcher_register,
    molt_asyncio_fd_watcher_unregister,
    molt_asyncio_gather_new,
    molt_asyncio_loop_enqueue_handle,
    molt_asyncio_ready_runner_new,
    molt_asyncio_running_loop_get,
    molt_asyncio_running_loop_set,
    molt_asyncio_timer_handle_cancel,
    molt_asyncio_timer_schedule,
    molt_asyncio_sock_accept_new,
    molt_asyncio_sock_connect_new,
    molt_asyncio_sock_recv_into_new,
    molt_asyncio_sock_recv_new,
    molt_asyncio_sock_recvfrom_into_new,
    molt_asyncio_sock_recvfrom_new,
    molt_asyncio_sock_sendall_new,
    molt_asyncio_sock_sendto_new,
    molt_asyncgen_shutdown,
    molt_block_on,
    molt_event_loop_add_reader,
    molt_event_loop_add_writer,
    molt_event_loop_call_at,
    molt_event_loop_call_later,
    molt_event_loop_call_soon,
    molt_event_loop_cancel_timer,
    molt_event_loop_close,
    molt_event_loop_drop,
    molt_event_loop_get_debug,
    molt_event_loop_get_exception_handler,
    molt_event_loop_get_task_factory,
    molt_event_loop_has_pending,
    molt_event_loop_is_closed,
    molt_event_loop_is_running,
    molt_event_loop_new,
    molt_event_loop_next_deadline_delay,
    molt_event_loop_notify_reader_ready,
    molt_event_loop_notify_writer_ready,
    molt_event_loop_ready_count,
    molt_event_loop_remove_reader,
    molt_event_loop_remove_writer,
    molt_event_loop_run_once,
    molt_event_loop_set_debug,
    molt_event_loop_set_exception_handler,
    molt_event_loop_set_task_factory,
    molt_event_loop_start,
    molt_event_loop_stop,
    molt_event_loop_time,
    molt_pipe_transport_close,
    molt_pipe_transport_drop,
    molt_pipe_transport_get_fd,
    molt_pipe_transport_get_write_buffer_size,
    molt_pipe_transport_is_closing,
    molt_pipe_transport_new,
    molt_pipe_transport_pause_reading,
    molt_pipe_transport_resume_reading,
    molt_pipe_transport_write,
    molt_task_register_token_owned,
    molt_thread_submit,
    open_connection,
    open_unix_connection,
    start_server,
    start_unix_server,
    wrap_future,
)
from . import protocols as protocols
from . import transports as transports
from .protocols import DatagramProtocol, Protocol
from .transports import DatagramTransport, Transport

if TYPE_CHECKING:
    from .streams import AbstractServer

_VERSION_INFO = getattr(sys, "version_info", (3, 12, 0, "final", 0))
_SOCKET = _asyncio._SOCKET
_socket = _SOCKET
socket = _SOCKET
_contextvars = contextvars
_os = os
_signal = signal
_subprocess = subprocess
_sys = sys
_threading = threading
_TYPE_CHECKING = TYPE_CHECKING

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
        if _DEBUG_ASYNCIO_HANDLES:
            cb = self._callback
            cb_name = getattr(cb, "__qualname__", None) or getattr(cb, "__name__", None)
            if cb_name is None:
                cb_name = type(cb).__name__
            _debug_write(f"asyncio_handle_run callback={cb_name}")
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
        if self.is_running():
            raise RuntimeError("Cannot close a running event loop")
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
        completed: Future | None = None
        try:
            if isinstance(future, Future):
                fut = future
                completed = fut
                if isinstance(fut, Task):
                    runner = getattr(fut, "_runner_task", None)
                    needs_runner = not getattr(fut, "_runner_spawned", True)
                    prev_token_id = _swap_current_token(fut._token)
                    try:
                        if needs_runner or runner is None:
                            runner = fut._runner(fut.get_coro())
                            fut._runner_task = runner
                            if molt_task_register_token_owned is not None:  # type: ignore[name-defined]
                                molt_task_register_token_owned(  # type: ignore[name-defined]
                                    runner, fut._token.token_id()
                                )
                        if getattr(fut, "_runner_spawned", True):
                            molt_block_on(fut._wait())
                        else:
                            molt_block_on(runner)
                        _debug_exc_state("run_until_complete_after_block_on")
                    finally:
                        _restore_token_id(prev_token_id)
                else:
                    molt_block_on(fut._wait())
                    _debug_exc_state("run_until_complete_after_wait")
            else:
                fut = Task(future, loop=self, _spawn_runner=False)
                completed = fut
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
                finally:
                    _restore_token_id(prev_token_id)
        except BaseException:
            _require_asyncio_intrinsic(molt_event_loop_stop, "event_loop_stop")(
                self._loop_handle
            )
            self._stopping = False
            _set_running_loop(prev)
            raise
        _require_asyncio_intrinsic(molt_event_loop_stop, "event_loop_stop")(
            self._loop_handle
        )
        self._stopping = False
        _set_running_loop(prev)
        _debug_exc_state("run_until_complete_return")
        if completed is None:
            return None
        result = Future.result(completed)
        _debug_exc_state("run_until_complete_after_result")
        return result

    def run_forever(self) -> None:
        # CPython-faithful imperative driver: each iteration runs one event-loop
        # turn (ready ``call_soon`` handles, due timers, and one scheduler drain
        # so awaited tasks advance) and then checks ``self._stopping``. This is a
        # direct port of ``BaseEventLoop.run_forever``'s
        # ``while True: self._run_once(); if self._stopping: break`` loop.
        #
        # The previous implementation drove a ``while not self._stopping: await
        # sleep(0)`` coroutine through ``run_until_complete``. That busy-wait
        # never observed a ``stop()`` scheduled from a ``call_soon`` callback
        # (the callback's turn was never reached), so ``_stopping`` never flipped
        # and each spin allocated a fresh sleep future -> unbounded allocation ->
        # OOM. Driving the loop directly makes the stop handshake deterministic
        # and the idle loop block instead of spin.
        if self.is_closed():
            raise RuntimeError("Event loop is closed")
        if self.is_running():
            raise RuntimeError("This event loop is already running")
        prev = _get_running_loop()
        _set_running_loop(self)
        _require_asyncio_intrinsic(molt_event_loop_start, "event_loop_start")(
            self._loop_handle
        )
        # NB: ``_stopping`` is intentionally NOT reset here. CPython's
        # ``run_forever`` only clears it in the ``finally`` block, so a
        # ``stop()`` issued before ``run_forever`` (``_stopping`` already True)
        # runs exactly one turn and returns -- matching
        # ``asyncio_run_forever_prestopped``.
        try:
            while True:
                ran = self._run_once()
                if self._stopping:
                    break
                # Only block when the turn did no work: an active loop (callbacks
                # still firing) advances immediately, while a genuinely idle loop
                # blocks instead of busy-spinning.
                if ran == 0:
                    self._run_forever_idle_wait()
        finally:
            self._stopping = False
            _require_asyncio_intrinsic(molt_event_loop_stop, "event_loop_stop")(
                self._loop_handle
            )
            _set_running_loop(prev)

    def _run_forever_idle_wait(self) -> None:
        # Block (never busy-spin) between turns. When a timer is scheduled, sleep
        # until just before its deadline; otherwise yield with a short bounded
        # sleep so external wakeups (worker threads completing tasks, I/O, timers
        # re-enqueued by the sleep worker) are observed promptly on the next turn.
        # Mirrors CPython blocking on the selector with the computed timeout.
        if self._stopping:
            return
        delay = self._next_deadline_delay()
        if delay <= 0.0:
            # Work is due right now (or a zero-delay timer/ready task is pending);
            # take the next turn immediately without sleeping.
            return
        # Cap the wait so a stop()/wakeup arriving from another thread is observed
        # without waiting out a long timer deadline.
        wait = delay if delay < _RUN_FOREVER_IDLE_CAP else _RUN_FOREVER_IDLE_CAP
        _time.sleep(wait)

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
        socket_module = _socket_module()
        if family == 0:
            family = socket_module.AF_INET
        sock = socket_module.socket(family, socket_module.SOCK_DGRAM, proto)
        sock.setblocking(False)
        if reuse_port and hasattr(socket_module, "SO_REUSEPORT"):
            sock.setsockopt(
                socket_module.SOL_SOCKET,
                int(getattr(socket_module, "SO_REUSEPORT")),
                1,
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
        return _socket_module().getaddrinfo(host, port, **kwargs)

    async def getnameinfo(self, sockaddr: Any, flags: int) -> Any:
        return _socket_module().getnameinfo(sockaddr, flags)

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
    if _DEBUG_ASYNCIO_SHUTDOWN:
        summaries = [_debug_task_summary(task) for task in tasks]
        _debug_write(
            "asyncio_cancel_all_tasks loop={loop_type} count={count} tasks={tasks}".format(
                loop_type=type(loop).__name__,
                count=len(tasks),
                tasks=summaries,
            )
        )
    if not tasks:
        return
    _asyncio_cancel_pending_tasks(tasks)
    try:
        waiter = _require_asyncio_intrinsic(
            molt_asyncio_gather_new, "asyncio_gather_new"
        )(tasks, True)
        if _DEBUG_ASYNCIO_SHUTDOWN:
            _debug_write(
                "asyncio_cancel_all_tasks_waiter {summary}".format(
                    summary=_debug_task_summary(waiter)
                )
            )
        loop.run_until_complete(waiter)
    except BaseException:
        if _DEBUG_ASYNCIO_SHUTDOWN:
            _debug_exc_state("cancel_all_tasks_after_waiter_exception")
        pass

def on_fork() -> None:
    global _CHILD_WATCHER
    _set_running_loop(None)
    _CHILD_WATCHER = None

format_helpers: Any | None = None
BaseDefaultEventLoopPolicy = DefaultEventLoopPolicy

__all__ = [
    "AbstractEventLoop",
    "AbstractServer",
    "Handle",
    "TimerHandle",
    "contextvars",
    "format_helpers",
    "get_event_loop",
    "get_event_loop_policy",
    "get_running_loop",
    "new_event_loop",
    "on_fork",
    "os",
    "set_child_watcher",
    "set_event_loop",
    "set_event_loop_policy",
    "signal",
    "socket",
    "subprocess",
    "sys",
    "threading",
]
if _VERSION_INFO < (3, 14):
    __all__.extend(["AbstractEventLoopPolicy", "BaseDefaultEventLoopPolicy"])
if _EXPOSE_CHILD_WATCHERS:
    __all__.extend(["get_child_watcher", "set_child_watcher"])

globals().pop("_require_intrinsic", None)
