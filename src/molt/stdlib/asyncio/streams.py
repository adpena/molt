"""Stream and socket-server authority for ``asyncio.streams``."""

from __future__ import annotations

import collections
import logging as _logging
import sys
import warnings
import weakref
from typing import TYPE_CHECKING, Any, Iterable

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from asyncio import (
    FlowControlMixin,
    IncompleteReadError,
    Protocol,
    _SOCKET,
    _errno,
    _io_wait,
    _map_socket_name_resolution_error,
    _molt_socket_reader_at_eof,
    _molt_socket_reader_drop,
    _molt_socket_reader_new,
    _molt_stream_close,
    _molt_stream_reader_at_eof,
    _molt_stream_reader_drop,
    _molt_stream_reader_new,
    _require_asyncio_intrinsic,
    _require_io_wait_new,
    _require_ssl_transport_support,
    _require_unix_socket_support,
    _socket_module,
    _socket_wait_key,
    _tls_client_connect,
    _tls_client_from_fd,
    _tls_server_from_fd,
    _tls_server_payload,
    get_running_loop,
    molt_asyncio_server_accept_loop_new,
    molt_asyncio_socket_reader_read_new,
    molt_asyncio_socket_reader_readline_new,
    molt_asyncio_stream_buffer_consume,
    molt_asyncio_stream_buffer_snapshot,
    molt_asyncio_stream_reader_read_new,
    molt_asyncio_stream_reader_readline_new,
    molt_asyncio_stream_send_all_new,
    sleep,
)

if TYPE_CHECKING:
    from asyncio import EventLoop

socket = _SOCKET
_socket = _SOCKET
logger = _logging.getLogger("asyncio")
coroutines: Any | None = None
events: Any | None = None
exceptions: Any | None = None
format_helpers: Any | None = None
protocols: Any | None = None

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
            self._sock.shutdown(_socket_module().SHUT_WR)

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

class StreamReaderProtocol(Protocol):
    """Stream reader protocol."""

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
    socket_module = _socket_module()
    sock = socket_module.socket(socket_module.AF_INET, socket_module.SOCK_STREAM)
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
    socket_module = _socket_module()
    sock = socket_module.socket(socket_module.AF_UNIX, socket_module.SOCK_STREAM)
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
            socket_type = _socket_module().socket

            def _tls_reader_ctor(conn: Any) -> ProcessStreamReader:
                if not isinstance(conn, socket_type):
                    raise TypeError(
                        "start_server ssl transport requires a stream socket connection"
                    )
                raw_fd = conn.detach()
                handle = _tls_server_from_fd(raw_fd, certfile, keyfile)
                tls_handles[id(conn)] = handle
                return ProcessStreamReader(handle)

            def _tls_writer_ctor(conn: Any) -> ProcessStreamWriter:
                if not isinstance(conn, socket_type):
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
    socket_module = _socket_module()
    sock = socket_module.socket(socket_module.AF_INET, socket_module.SOCK_STREAM)
    sock.setsockopt(socket_module.SOL_SOCKET, socket_module.SO_REUSEADDR, 1)
    if reuse_port and hasattr(socket_module, "SO_REUSEPORT"):
        sock.setsockopt(
            socket_module.SOL_SOCKET,
            int(getattr(socket_module, "SO_REUSEPORT")),
            1,
        )
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
            socket_type = _socket_module().socket

            def _tls_reader_ctor(conn: Any) -> ProcessStreamReader:
                if not isinstance(conn, socket_type):
                    raise TypeError(
                        "start_unix_server ssl transport requires a stream socket connection"
                    )
                raw_fd = conn.detach()
                handle = _tls_server_from_fd(raw_fd, certfile, keyfile)
                tls_handles[id(conn)] = handle
                return ProcessStreamReader(handle)

            def _tls_writer_ctor(conn: Any) -> ProcessStreamWriter:
                if not isinstance(conn, socket_type):
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
    socket_module = _socket_module()
    sock = socket_module.socket(socket_module.AF_UNIX, socket_module.SOCK_STREAM)
    sock.setblocking(False)
    sock.bind(path)
    sock.listen(backlog)
    return Server(
        sock, client_connected_cb, reader_ctor=reader_ctor, writer_ctor=writer_ctor
    )


__all__ = [
    "FlowControlMixin",
    "StreamReader",
    "StreamReaderProtocol",
    "StreamWriter",
    "collections",
    "coroutines",
    "events",
    "exceptions",
    "format_helpers",
    "logger",
    "open_connection",
    "open_unix_connection",
    "protocols",
    "sleep",
    "socket",
    "start_server",
    "start_unix_server",
    "sys",
    "warnings",
    "weakref",
]

globals().pop("_require_intrinsic", None)
