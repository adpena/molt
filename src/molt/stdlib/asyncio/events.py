"""Event-loop, policy, watcher, and transport-shim authority for ``asyncio.events``."""

from __future__ import annotations

import contextvars
from _compatibility_errors import (
    abstract_add_signal_handler_error as _abstract_add_signal_handler_error,
    abstract_remove_signal_handler_error as _abstract_remove_signal_handler_error,
    windows_add_signal_handler_error as _windows_add_signal_handler_error,
    windows_remove_signal_handler_error as _windows_remove_signal_handler_error,
)
import errno as _errno
import os
import signal
import subprocess
import sys
import threading
import warnings as _warnings
import weakref as _weakref
from typing import TYPE_CHECKING, Any, Callable, cast as _cast

from _intrinsics import require_intrinsic as _require_intrinsic
_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

import asyncio as _asyncio
from asyncio import (
    Future,
    ProcessStreamWriter,
    StreamReader,
    StreamWriter,
    Task,
    _EXPOSE_CHILD_WATCHERS,
    _EXPOSE_WINDOWS_POLICIES,
    _IS_WINDOWS,
    _asyncio_cancel_pending_tasks,
    _fd_from_fileobj,
    _require_asyncio_intrinsic,
    _require_child_watcher_support,
    _require_ssl_transport_support,
    _socket_wait_key,
    _socket_module,
    _tls_client_from_fd,
    _tls_server_from_fd,
    _tls_server_payload,
    all_tasks,
    gather,
    create_subprocess_exec,
    create_subprocess_shell,
    _molt_asyncio_child_watcher_add,
    _molt_asyncio_child_watcher_clear,
    _molt_asyncio_child_watcher_pop,
    _molt_asyncio_child_watcher_remove,
    _molt_asyncio_event_loop_get_current,
    _molt_asyncio_event_loop_policy_get,
    _molt_asyncio_event_loop_policy_set,
    _molt_asyncio_event_loop_set,
    _molt_asyncio_fd_watcher_register,
    _molt_asyncio_fd_watcher_unregister,
    _molt_asyncio_running_loop_get,
    _molt_asyncio_running_loop_set,
    _molt_asyncio_sock_accept_new,
    _molt_asyncio_sock_connect_new,
    _molt_asyncio_sock_recv_into_new,
    _molt_asyncio_sock_recv_new,
    _molt_asyncio_sock_recvfrom_into_new,
    _molt_asyncio_sock_recvfrom_new,
    _molt_asyncio_sock_sendall_new,
    _molt_asyncio_sock_sendto_new,
    _molt_event_loop_add_reader,
    _molt_event_loop_add_writer,
    _molt_event_loop_call_at,
    _molt_event_loop_call_soon,
    _molt_event_loop_cancel_timer,
    _molt_event_loop_close,
    _molt_event_loop_drop,
    _molt_event_loop_get_debug,
    _molt_event_loop_get_exception_handler,
    _molt_event_loop_get_task_factory,
    _molt_event_loop_is_closed,
    _molt_event_loop_is_running,
    _molt_event_loop_new,
    _molt_event_loop_notify_reader_ready,
    _molt_event_loop_notify_writer_ready,
    _molt_event_loop_remove_reader,
    _molt_event_loop_remove_writer,
    _molt_event_loop_run_once,
    _molt_event_loop_set_debug,
    _molt_event_loop_set_exception_handler,
    _molt_event_loop_set_task_factory,
    _molt_event_loop_spawn,
    _molt_event_loop_start,
    _molt_event_loop_stop,
    _molt_event_loop_time,
    _molt_event_loop_wait,
    _molt_event_loop_wake,
    _molt_pipe_transport_close,
    _molt_pipe_transport_drop,
    _molt_pipe_transport_get_fd,
    _molt_pipe_transport_get_write_buffer_size,
    _molt_pipe_transport_is_closing,
    _molt_pipe_transport_new,
    _molt_pipe_transport_pause_reading,
    _molt_pipe_transport_resume_reading,
    _molt_pipe_transport_write,
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
        self._context = _contextvars.copy_context() if context is None else context
        self._cancelled = False

    def cancel(self) -> None:
        self._cancelled = True
        self._callback = None
        self._args = None

    def cancelled(self) -> bool:
        return self._cancelled

    def _run(self) -> None:
        if self._cancelled:
            return
        try:
            self._context.run(self._callback, *self._args)
        except (SystemExit, KeyboardInterrupt):
            raise
        except BaseException as exc:
            self._loop.call_exception_handler({
                "message": f"Exception in callback {self._callback!r}",
                "exception": exc,
                "handle": self,
            })

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
        self._timer_id: Any | None = None

    def when(self) -> float:
        return self._when

    def cancel(self) -> None:
        if self._cancelled:
            return
        super().cancel()
        timer_id = self._timer_id
        self._timer_id = None
        if timer_id is not None:
            self._loop._cancel_rust_timer(timer_id)

def _write_exception_message(message: str) -> None:
    err = getattr(sys, "stderr", None)
    if err is None or not hasattr(err, "write"):
        err = getattr(sys, "__stderr__", None)
    if err is not None and hasattr(err, "write"):
        err.write(f"{message}\n")
        flush_fn = getattr(err, "flush", None)
        if callable(flush_fn):
            flush_fn()
        return None
    out = getattr(sys, "stdout", None)
    if out is not None and hasattr(out, "write"):
        out.write(f"{message}\n")
        flush_fn = getattr(out, "flush", None)
        if callable(flush_fn):
            flush_fn()
        return None
    print(message)


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

    async def shutdown_default_executor(self, timeout=None) -> None:
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
        raise _abstract_add_signal_handler_error()

    def remove_signal_handler(self, sig: int) -> bool:
        raise _abstract_remove_signal_handler_error()

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

def _signal_dispatcher(loop_ref: Any) -> Callable[[int, Any], None]:
    """Python-level handler installed by ``add_signal_handler``.

    It holds its loop weakly: the process signal table never keeps a loop
    alive. A delivery for a collected loop retires the handler, as that loop's
    ``close()`` would have; that one delivery is not re-raised.
    """

    def _dispatch_loop_signal(signum: int, frame: Any) -> None:
        loop = loop_ref()
        if loop is not None:
            loop._handle_signal(signum)
            return
        _signal.signal(
            signum,
            _signal.default_int_handler if signum == _signal.SIGINT else _signal.SIG_DFL,
        )

    return _dispatch_loop_signal

class _EventLoop(AbstractEventLoop):
    def __init__(self, selector: Any | None = None) -> None:
        # Rust owns callback and timer custody for every loop driver.
        self._loop_handle: Any = _require_asyncio_intrinsic(
            _molt_event_loop_new, "event_loop_new"
        )()
        self._readers: dict[int, tuple[Any, tuple[Any, ...], Task]] = {}
        self._writers: dict[int, tuple[Any, tuple[Any, ...], Task]] = {}
        self._asyncgens = _weakref.WeakSet()
        self._asyncgens_shutdown_called = False
        self._stopping = False
        self._default_executor: Any | None = None
        self._executor_shutdown_called = False
        self._selector = selector
        self._signal_handlers: dict[int, Handle] = {}

    def __del__(self) -> None:
        handle = getattr(self, "_loop_handle", None)
        if handle is not None and _molt_event_loop_drop is not None:  # type: ignore[name-defined]
            try:
                _molt_event_loop_drop(handle)  # type: ignore[name-defined]
            except Exception:
                pass

    def create_future(self) -> Future:
        return Future(loop=self)

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

    def _spawn_task(self, runner: Any) -> None:
        _require_asyncio_intrinsic(_molt_event_loop_spawn, "event_loop_spawn")(
            self._loop_handle, runner
        )

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
        _require_asyncio_intrinsic(_molt_event_loop_call_soon, "event_loop_call_soon")(
            self._loop_handle, handle
        )
        return handle

    def call_soon_threadsafe(
        self, callback: Callable[..., Any], /, *args: Any, context: Any | None = None
    ) -> Handle:
        # Every Rust ready-queue publication signals a parked loop, so the
        # thread-safe variant needs no separate self-pipe write.
        return self.call_soon(callback, *args, context=context)

    def call_later(
        self, delay: float, callback: Callable[..., Any], /, *args: Any,
        context: Any | None = None,
    ) -> TimerHandle:
        return self.call_at(self.time() + float(delay), callback, *args, context=context)

    def call_at(
        self, when: float, callback: Callable[..., Any], /, *args: Any,
        context: Any | None = None,
    ) -> TimerHandle:
        if self.is_closed():
            raise RuntimeError("Event loop is closed")
        handle = TimerHandle(float(when), callback, args, self, context)
        handle._timer_id = _require_asyncio_intrinsic(
            _molt_event_loop_call_at, "event_loop_call_at"
        )(self._loop_handle, float(when), handle)
        return handle

    def set_exception_handler(
        self, handler: Callable[["EventLoop", dict[str, Any]], Any] | None
    ) -> None:
        if handler is not None and not callable(handler):
            raise TypeError("A callable object or None is expected")
        _require_asyncio_intrinsic(
            _molt_event_loop_set_exception_handler, "event_loop_set_exception_handler"
        )(self._loop_handle, handler)

    def get_exception_handler(
        self,
    ) -> Callable[["EventLoop", dict[str, Any]], Any] | None:
        return _require_asyncio_intrinsic(
            _molt_event_loop_get_exception_handler, "event_loop_get_exception_handler"
        )(self._loop_handle)

    def default_exception_handler(self, context: dict[str, Any]) -> None:
        message = context.get("message", "Unhandled exception in event loop")
        exc = context.get("exception")
        _write_exception_message(message if exc is None else f"{message}: {exc}")

    def call_exception_handler(self, context: dict[str, Any]) -> None:
        handler = self.get_exception_handler()
        try:
            if handler is None:
                self.default_exception_handler(context)
            else:
                source = context.get("task") or context.get("future") or context.get("handle")
                callback_context = getattr(source, "_context", None)
                if callback_context is None:
                    handler(self, context)
                else:
                    callback_context.run(handler, self, context)
        except (SystemExit, KeyboardInterrupt):
            raise
        except BaseException as exc:
            try:
                self.default_exception_handler({
                    "message": "Unhandled error in exception handler",
                    "exception": exc,
                    "context": context,
                })
            except (SystemExit, KeyboardInterrupt):
                raise
            except BaseException as error:
                _write_exception_message(f"Exception in default exception handler: {error}")

    def set_debug(self, enabled: bool) -> None:
        _require_asyncio_intrinsic(_molt_event_loop_set_debug, "event_loop_set_debug")(
            self._loop_handle, bool(enabled)
        )

    def get_debug(self) -> bool:
        return bool(
            _require_asyncio_intrinsic(
                _molt_event_loop_get_debug, "event_loop_get_debug"
            )(self._loop_handle)
        )

    def set_task_factory(self, factory: Callable[..., Task] | None) -> None:
        _require_asyncio_intrinsic(
            _molt_event_loop_set_task_factory, "event_loop_set_task_factory"
        )(self._loop_handle, factory)

    def get_task_factory(self) -> Callable[..., Task] | None:
        return _require_asyncio_intrinsic(
            _molt_event_loop_get_task_factory, "event_loop_get_task_factory"
        )(self._loop_handle)

    def time(self) -> float:
        return float(
            _require_asyncio_intrinsic(_molt_event_loop_time, "event_loop_time")(
                self._loop_handle
            )
        )

    def is_running(self) -> bool:
        return bool(
            _require_asyncio_intrinsic(
                _molt_event_loop_is_running, "event_loop_is_running"
            )(self._loop_handle)
        )

    def is_closed(self) -> bool:
        return bool(
            _require_asyncio_intrinsic(
                _molt_event_loop_is_closed, "event_loop_is_closed"
            )(self._loop_handle)
        )

    def stop(self) -> None:
        self._stopping = True
        # The request lives outside the Rust queues: wake a parked loop so it
        # observes it. Stopping a retired loop is a no-op, as in CPython.
        _require_asyncio_intrinsic(_molt_event_loop_wake, "event_loop_wake")(
            self._loop_handle
        )

    def close(self) -> None:
        if self.is_running():
            raise RuntimeError("Cannot close a running event loop")
        if self.is_closed():
            return
        _require_asyncio_intrinsic(_molt_event_loop_close, "event_loop_close")(
            self._loop_handle
        )
        self._executor_shutdown_called = True
        executor = self._default_executor
        self._default_executor = None
        if executor is not None:
            executor.shutdown(wait=False)
        if self._selector is not None and hasattr(self._selector, "close"):
            self._selector.close()
        # CPython `_UnixSelectorEventLoop.close`: a closed loop cannot run what
        # its signal handlers schedule, so they are removed with it.
        if self._signal_handlers:
            if _sys.is_finalizing():
                _warnings.warn(
                    f"Closing the loop {self!r} on interpreter shutdown stage, "
                    "skipping signal handlers removal",
                    ResourceWarning,
                    source=self,
                )
                self._signal_handlers.clear()
            else:
                for sig in list(self._signal_handlers):
                    self.remove_signal_handler(sig)

    def run_in_executor(self, executor, func, *args):
        if self.is_closed():
            raise RuntimeError("Event loop is closed")
        if executor is None:
            if self._executor_shutdown_called:
                raise RuntimeError("Executor shutdown has been called")
            executor = self._default_executor
            if executor is None:
                executor = _asyncio._concurrent.futures.ThreadPoolExecutor(
                    thread_name_prefix="asyncio"
                )
                self._default_executor = executor
        return wrap_future(executor.submit(func, *args), loop=self)

    def add_reader(self, fd: Any, callback: Any, *args: Any) -> None:
        fileno = _fd_from_fileobj(fd)
        # Register with Rust event loop for I/O readiness notification.
        _require_asyncio_intrinsic(_molt_event_loop_add_reader, "event_loop_add_reader")(
            self._loop_handle, fileno, Handle(callback, args, self, None)
        )
        _require_asyncio_intrinsic(
            _molt_asyncio_fd_watcher_register, "asyncio_fd_watcher_register"
        )(self, self._readers, fileno, self._notify_reader_ready, (fileno,), 1)

    def remove_reader(self, fd: Any) -> bool:
        fileno = _fd_from_fileobj(fd)
        _require_asyncio_intrinsic(
            _molt_event_loop_remove_reader, "event_loop_remove_reader"
        )(self._loop_handle, fileno)
        return bool(
            _require_asyncio_intrinsic(
                _molt_asyncio_fd_watcher_unregister, "asyncio_fd_watcher_unregister"
            )(self._readers, fileno)
        )

    def add_writer(self, fd: Any, callback: Any, *args: Any) -> None:
        fileno = _fd_from_fileobj(fd)
        # Register with Rust event loop for I/O writability notification.
        _require_asyncio_intrinsic(_molt_event_loop_add_writer, "event_loop_add_writer")(
            self._loop_handle, fileno, Handle(callback, args, self, None)
        )
        _require_asyncio_intrinsic(
            _molt_asyncio_fd_watcher_register, "asyncio_fd_watcher_register"
        )(self, self._writers, fileno, self._notify_writer_ready, (fileno,), 2)

    async def sock_recv(self, sock: Any, n: int) -> bytes:
        fut = _require_asyncio_intrinsic(
            _molt_asyncio_sock_recv_new, "asyncio_sock_recv_new"
        )(sock, n, _socket_wait_key(sock))
        return await fut

    async def sock_recv_into(self, sock: Any, buf: Any) -> int:
        nbytes = len(buf)
        fut = _require_asyncio_intrinsic(
            _molt_asyncio_sock_recv_into_new, "asyncio_sock_recv_into_new"
        )(sock, buf, nbytes, _socket_wait_key(sock))
        return await fut

    async def sock_sendall(self, sock: Any, data: bytes) -> None:
        fut = _require_asyncio_intrinsic(
            _molt_asyncio_sock_sendall_new, "asyncio_sock_sendall_new"
        )(sock, data, _socket_wait_key(sock))
        await fut

    async def sock_recvfrom(self, sock: Any, bufsize: int) -> tuple[Any, Any]:
        fut = _require_asyncio_intrinsic(
            _molt_asyncio_sock_recvfrom_new, "asyncio_sock_recvfrom_new"
        )(sock, bufsize, _socket_wait_key(sock))
        return await fut

    async def sock_recvfrom_into(self, sock: Any, buf: Any) -> tuple[int, Any]:
        nbytes = len(buf)
        fut = _require_asyncio_intrinsic(
            _molt_asyncio_sock_recvfrom_into_new, "asyncio_sock_recvfrom_into_new"
        )(sock, buf, nbytes, _socket_wait_key(sock))
        return await fut

    async def sock_sendto(self, sock: Any, data: bytes, addr: Any) -> int:
        fut = _require_asyncio_intrinsic(
            _molt_asyncio_sock_sendto_new, "asyncio_sock_sendto_new"
        )(sock, data, addr, _socket_wait_key(sock))
        return await fut

    async def sock_connect(self, sock: Any, address: Any) -> None:
        fut = _require_asyncio_intrinsic(
            _molt_asyncio_sock_connect_new, "asyncio_sock_connect_new"
        )(sock, address, _socket_wait_key(sock))
        await fut

    async def sock_accept(self, sock: Any) -> tuple[Any, Any]:
        fut = _require_asyncio_intrinsic(
            _molt_asyncio_sock_accept_new, "asyncio_sock_accept_new"
        )(sock, _socket_wait_key(sock))
        return await fut

    def remove_writer(self, fd: Any) -> bool:
        fileno = _fd_from_fileobj(fd)
        _require_asyncio_intrinsic(
            _molt_event_loop_remove_writer, "event_loop_remove_writer"
        )(self._loop_handle, fileno)
        return bool(
            _require_asyncio_intrinsic(
                _molt_asyncio_fd_watcher_unregister, "asyncio_fd_watcher_unregister"
            )(self._writers, fileno)
        )

    def _run_once(self) -> int:
        """Run one iteration of the Rust event loop (hot path).

        Delegates entirely to ``molt_event_loop_run_once`` which handles
        selector poll, timer firing, and ready-queue drain in Rust.
        Returns the number of callbacks executed (0 means idle).
        """
        return int(
            _require_asyncio_intrinsic(_molt_event_loop_run_once, "event_loop_run_once")(
                self._loop_handle
            )
        )

    def _cancel_rust_timer(self, timer_id: Any) -> None:
        """Cancel a Rust-level timer by the opaque timer_id returned from call_later/call_at."""
        _require_asyncio_intrinsic(
            _molt_event_loop_cancel_timer, "event_loop_cancel_timer"
        )(self._loop_handle, timer_id)

    def _notify_reader_ready(self, fd: int) -> None:
        """Notify the Rust event loop that *fd* is readable.

        Called by transport/protocol glue when the selector reports readability
        outside of the normal Rust poll path.
        """
        _require_asyncio_intrinsic(
            _molt_event_loop_notify_reader_ready, "event_loop_notify_reader_ready"
        )(self._loop_handle, fd)

    def _notify_writer_ready(self, fd: int) -> None:
        """Notify the Rust event loop that *fd* is writable."""
        _require_asyncio_intrinsic(
            _molt_event_loop_notify_writer_ready, "event_loop_notify_writer_ready"
        )(self._loop_handle, fd)

    def _check_running(self) -> None:
        if self.is_closed():
            raise RuntimeError("Event loop is closed")
        if self.is_running():
            raise RuntimeError("This event loop is already running")
        if _get_running_loop() is not None:
            raise RuntimeError("Cannot run the event loop while another loop is running")

    def run_until_complete(self, future: Any) -> Any:
        self._check_running()
        completed = _asyncio.ensure_future(future, loop=self)

        def stop_when_done(done):
            if not done.cancelled() and isinstance(done.exception(), (SystemExit, KeyboardInterrupt)):
                return
            self.stop()

        completed.add_done_callback(stop_when_done)
        try:
            self.run_forever()
        finally:
            completed.remove_done_callback(stop_when_done)
        if not completed.done():
            raise RuntimeError("Event loop stopped before Future completed.")
        return completed.result()

    def run_forever(self) -> None:
        self._check_running()
        previous_hooks = sys.get_asyncgen_hooks()
        _set_running_loop(self)
        _require_asyncio_intrinsic(_molt_event_loop_start, "event_loop_start")(
            self._loop_handle
        )
        try:
            sys.set_asyncgen_hooks(
                firstiter=self._asyncgen_firstiter_hook,
                finalizer=self._asyncgen_finalizer_hook,
            )
            # A pre-stopped loop still executes one turn. Callbacks queued by that
            # turn remain owned by this loop for its next invocation.
            while True:
                ran = self._run_once()
                if self._stopping:
                    break
                if ran == 0:
                    self._run_forever_idle_wait()
        finally:
            self._stopping = False
            _require_asyncio_intrinsic(_molt_event_loop_stop, "event_loop_stop")(
                self._loop_handle
            )
            _set_running_loop(None)
            sys.set_asyncgen_hooks(*previous_hooks)

    def _run_forever_idle_wait(self) -> None:
        # Rust parks this thread until ready work, the earliest deadline,
        # stop(), close, or a signal delivery for the main thread; native parks
        # release the GIL. Python signal handlers run at the safepoint that
        # follows this call's return.
        _require_asyncio_intrinsic(_molt_event_loop_wait, "event_loop_wait")(
            self._loop_handle
        )

    def _asyncgen_firstiter_hook(self, generator):
        if self._asyncgens_shutdown_called:
            _warnings.warn(
                'Asynchronous generator {!r} was scheduled after '
                'loop.shutdown_asyncgens() call'.format(generator),
                ResourceWarning,
                source=self,
            )
        self._asyncgens.add(generator)

    def _asyncgen_finalizer_hook(self, generator):
        self._asyncgens.discard(generator)
        if not self.is_closed():
            self.call_soon_threadsafe(self._close_asyncgen, generator)

    def _close_asyncgen(self, generator):
        _asyncio.ensure_future(generator.aclose(), loop=self)

    async def shutdown_asyncgens(self):
        self._asyncgens_shutdown_called = True
        if not self._asyncgens:
            return
        # The WeakSet snapshot creates real owned references before any close callback.
        generators = list(self._asyncgens)
        self._asyncgens.clear()
        results = await gather(
            *(generator.aclose() for generator in generators),
            return_exceptions=True,
        )
        for result, generator in zip(results, generators):
            if isinstance(result, BaseException):
                self.call_exception_handler({
                    'message': 'an error occurred during closing of asynchronous generator {!r}'.format(generator),
                    'exception': result,
                    'asyncgen': generator,
                })

    async def shutdown_default_executor(self, timeout=None):
        self._executor_shutdown_called = True
        executor = self._default_executor
        if executor is None:
            return
        finished = self.create_future()

        def complete(exception):
            if not finished.done():
                if exception is None:
                    finished.set_result(None)
                else:
                    finished.set_exception(exception)

        def join_executor():
            exception = None
            try:
                executor.shutdown(wait=True)
            except BaseException as error:
                exception = error
            if not self.is_closed():
                self.call_soon_threadsafe(complete, exception)

        thread = threading.Thread(target=join_executor)
        thread.start()
        try:
            async with _asyncio.timeout(timeout):
                await finished
        except TimeoutError:
            _warnings.warn("The executor did not finish joining within the timeout.",
                           RuntimeWarning, stacklevel=2)
            executor.shutdown(wait=False)
        else:
            thread.join()

    def set_default_executor(self, executor):
        if not isinstance(executor, _asyncio._concurrent.futures.ThreadPoolExecutor):
            raise TypeError("executor must be ThreadPoolExecutor instance")
        self._default_executor = executor

    def add_signal_handler(
        self, sig: int, callback: Callable[..., Any], /, *args: Any
    ) -> None:
        """Register *callback* to be called when signal *sig* is received.

        Unix only, as in CPython: Windows and WASM loops raise
        ``NotImplementedError``. The callback is bound now, in the current
        context, and each delivery schedules it on this loop.
        """
        if _IS_WINDOWS:
            raise _windows_add_signal_handler_error()
        if _sys.platform in ("emscripten", "wasi"):
            raise NotImplementedError(
                "signal handlers are not supported on this platform"
            )
        if _asyncio.iscoroutine(callback) or _asyncio.iscoroutinefunction(callback):
            raise TypeError("coroutines cannot be used with add_signal_handler()")
        self._check_signal(sig)
        if self.is_closed():
            raise RuntimeError("Event loop is closed")
        if _threading.current_thread() is not _threading.main_thread():
            # CPython reports the main-thread requirement through
            # signal.set_wakeup_fd(), which it calls at this point.
            raise RuntimeError(
                "set_wakeup_fd only works in main thread of the main interpreter"
            )
        self._signal_handlers[sig] = Handle(callback, args, self, None)
        try:
            _signal.signal(sig, _signal_dispatcher(_weakref.ref(self)))
        except ValueError as exc:
            del self._signal_handlers[sig]
            raise RuntimeError(str(exc))
        except OSError as exc:
            del self._signal_handlers[sig]
            if exc.errno == _errno.EINVAL:
                raise RuntimeError(f"sig {sig:d} cannot be caught")
            raise

    def remove_signal_handler(self, sig: int) -> bool:
        """Remove the handler for signal *sig*; return whether one was set.

        Unix only, as in CPython. SIGINT returns to ``default_int_handler``,
        every other signal to ``SIG_DFL``.
        """
        if _IS_WINDOWS:
            raise _windows_remove_signal_handler_error()
        if _sys.platform in ("emscripten", "wasi"):
            raise NotImplementedError(
                "signal handlers are not supported on this platform"
            )
        self._check_signal(sig)
        if self._signal_handlers.pop(sig, None) is None:
            return False
        handler = (
            _signal.default_int_handler if sig == _signal.SIGINT else _signal.SIG_DFL
        )
        try:
            _signal.signal(sig, handler)
        except OSError as exc:
            if exc.errno == _errno.EINVAL:
                raise RuntimeError(f"sig {sig:d} cannot be caught")
            raise
        return True

    def _check_signal(self, sig: Any) -> None:
        if not isinstance(sig, int):
            raise TypeError(f"sig must be an int, not {sig!r}")
        if sig not in _signal.valid_signals():
            raise ValueError(f"invalid signal number {sig}")

    def _handle_signal(self, sig: int) -> None:
        """Schedule the Handle bound to *sig* (CPython ``_handle_signal``)."""
        handle = self._signal_handlers.get(sig)
        if handle is None or self.is_closed():
            return  # A delivery racing removal or close.
        if handle._cancelled:
            self.remove_signal_handler(sig)
            return
        # The same Handle, with its registration-time context, is queued for
        # each delivery; queuing wakes a parked loop.
        _require_asyncio_intrinsic(_molt_event_loop_call_soon, "event_loop_call_soon")(
            self._loop_handle, handle
        )

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
            _molt_pipe_transport_new, "pipe_transport_new"
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
            _molt_pipe_transport_get_fd, "pipe_transport_get_fd"
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
            _molt_pipe_transport_new, "pipe_transport_new"
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
        loop = _molt_asyncio_event_loop_get_current()
        if _TYPE_CHECKING:
            return _cast(EventLoop, loop)
        return loop

    def set_event_loop(self, loop: EventLoop | None) -> None:
        _molt_asyncio_event_loop_set(loop)

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
            _molt_asyncio_child_watcher_add, "asyncio_child_watcher_add"
        )(self._callbacks, int(pid), callback, args)

    def remove_child_handler(self, pid: int) -> bool:
        return bool(
            _require_asyncio_intrinsic(
                _molt_asyncio_child_watcher_remove, "asyncio_child_watcher_remove"
            )(self._callbacks, int(pid))
        )

    def close(self) -> None:
        _require_asyncio_intrinsic(
            _molt_asyncio_child_watcher_clear, "asyncio_child_watcher_clear"
        )(self._callbacks)
        self._loop = None

    def is_active(self) -> bool:
        return self._loop is not None

    def _notify_child_exit(self, pid: int, returncode: int) -> None:
        entry = _require_asyncio_intrinsic(
            _molt_asyncio_child_watcher_pop, "asyncio_child_watcher_pop"
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
            _molt_pipe_transport_is_closing, "pipe_transport_is_closing"
        )
        return bool(is_closing_fn(self._pipe_handle))

    def close(self) -> None:
        if self._closing:
            return
        self._closing = True
        close_fn = _require_asyncio_intrinsic(
            _molt_pipe_transport_close, "pipe_transport_close"
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
            _molt_pipe_transport_pause_reading, "pipe_transport_pause_reading"
        )
        pause_fn(self._pipe_handle)

    def resume_reading(self) -> None:
        if not self._paused or self._closing:
            return
        self._paused = False
        resume_fn = _require_asyncio_intrinsic(
            _molt_pipe_transport_resume_reading, "pipe_transport_resume_reading"
        )
        resume_fn(self._pipe_handle)

    def get_pid(self) -> int | None:
        return None

    def get_pipe(self) -> Any:
        return self._pipe

    def __del__(self) -> None:
        drop_fn = _require_asyncio_intrinsic(
            _molt_pipe_transport_drop, "pipe_transport_drop"
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
            _molt_pipe_transport_is_closing, "pipe_transport_is_closing"
        )
        return bool(is_closing_fn(self._pipe_handle))

    def write(self, data: bytes) -> None:
        if self._closing:
            raise RuntimeError("transport is closing")
        if not data:
            return
        write_fn = _require_asyncio_intrinsic(
            _molt_pipe_transport_write, "pipe_transport_write"
        )
        write_fn(self._pipe_handle, data)

    def write_eof(self) -> None:
        self.close()

    def can_write_eof(self) -> bool:
        return True

    def get_write_buffer_size(self) -> int:
        buf_fn = _require_asyncio_intrinsic(
            _molt_pipe_transport_get_write_buffer_size,
            "pipe_transport_get_write_buffer_size",
        )
        return int(buf_fn(self._pipe_handle))

    def close(self) -> None:
        if self._closing:
            return
        self._closing = True
        close_fn = _require_asyncio_intrinsic(
            _molt_pipe_transport_close, "pipe_transport_close"
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
            _molt_pipe_transport_drop, "pipe_transport_drop"
        )
        drop_fn(self._pipe_handle)

def _get_running_loop() -> EventLoop | None:
    return _molt_asyncio_running_loop_get()

def _set_running_loop(loop: EventLoop | None) -> None:
    _molt_asyncio_running_loop_set(loop)

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
    policy = _molt_asyncio_event_loop_policy_get()
    if policy is None:
        policy = _default_event_loop_policy()
        _molt_asyncio_event_loop_policy_set(policy)
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
    _molt_asyncio_event_loop_policy_set(policy)

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
    tasks = list(all_tasks(loop))
    if not tasks:
        return
    _asyncio_cancel_pending_tasks(tasks)
    waiter = gather(*tasks, return_exceptions=True)
    loop.run_until_complete(waiter)
    for task in tasks:
        if task.cancelled():
            continue
        error = task.exception()
        if error is not None:
            loop.call_exception_handler({
                "message": "unhandled exception during asyncio.run() shutdown",
                "exception": error,
                "task": task,
            })

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
