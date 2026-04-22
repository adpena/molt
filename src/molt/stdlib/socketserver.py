"""Intrinsic-backed socketserver surface for Molt."""

from __future__ import annotations

from abc import ABCMeta as _ABCMeta
from io import BufferedIOBase
import os
import selectors
import socket
import sys
import threading
import time as _time_module
from typing import Any as _Any

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe")
_MOLT_SOCKETSERVER_SERVE_FOREVER = _require_intrinsic("molt_socketserver_serve_forever")
_MOLT_SOCKETSERVER_HANDLE_REQUEST = _require_intrinsic(
    "molt_socketserver_handle_request"
)
_MOLT_SOCKETSERVER_SHUTDOWN = _require_intrinsic("molt_socketserver_shutdown")
_MOLT_SOCKETSERVER_REGISTER = _require_intrinsic("molt_socketserver_register")
_MOLT_SOCKETSERVER_UNREGISTER = _require_intrinsic("molt_socketserver_unregister")
_MOLT_SOCKETSERVER_DISPATCH_BEGIN = _require_intrinsic(
    "molt_socketserver_dispatch_begin"
)
_MOLT_SOCKETSERVER_DISPATCH_POLL = _require_intrinsic("molt_socketserver_dispatch_poll")
_MOLT_SOCKETSERVER_DISPATCH_CANCEL = _require_intrinsic(
    "molt_socketserver_dispatch_cancel"
)
_MOLT_SOCKETSERVER_GET_REQUEST_POLL = _require_intrinsic(
    "molt_socketserver_get_request_poll"
)
_MOLT_SOCKETSERVER_SET_RESPONSE = _require_intrinsic("molt_socketserver_set_response")


# CPython exports `time` as the monotonic builtin.
time = _require_intrinsic("molt_time_monotonic")

if type(BufferedIOBase).__name__ != "ABCMeta":

    class _BufferedIOBaseShim(metaclass=_ABCMeta):
        pass

    BufferedIOBase = _BufferedIOBaseShim


class _FakeSocket:
    def __init__(self, request_bytes: bytes) -> None:
        self._request = request_bytes
        self._read_pos = 0
        self._response = bytearray()

    def recv(self, size: int) -> bytes:
        if size <= 0:
            size = 4096
        if self._read_pos >= len(self._request):
            return b""
        end = min(len(self._request), self._read_pos + size)
        out = self._request[self._read_pos : end]
        self._read_pos = end
        return out

    def sendall(self, data: bytes | bytearray | memoryview) -> None:
        self._response.extend(bytes(data))

    def response_bytes(self) -> bytes:
        return bytes(self._response)

    def close(self) -> None:
        return None


class _SocketReader:
    def __init__(self, conn: _Any) -> None:
        self._conn = conn
        self._buf = bytearray()

    def readline(self, size: int = -1) -> bytes:
        if size == 0:
            return b""
        limit = size if size is not None and size > 0 else -1
        while True:
            nl = self._buf.find(b"\n")
            if nl != -1:
                end = nl + 1
                if limit > 0:
                    end = min(end, limit)
                out = bytes(self._buf[:end])
                del self._buf[:end]
                return out
            if limit > 0 and len(self._buf) >= limit:
                out = bytes(self._buf[:limit])
                del self._buf[:limit]
                return out
            chunk = self._conn.recv(4096)
            if not chunk:
                out = bytes(self._buf)
                self._buf.clear()
                return out
            self._buf.extend(chunk)

    def read(self, size: int = -1) -> bytes:
        if size is not None and size >= 0:
            while len(self._buf) < size:
                chunk = self._conn.recv(4096)
                if not chunk:
                    break
                self._buf.extend(chunk)
            out = bytes(self._buf[:size])
            del self._buf[:size]
            return out
        chunks = [bytes(self._buf)] if self._buf else []
        self._buf.clear()
        while True:
            chunk = self._conn.recv(4096)
            if not chunk:
                break
            chunks.append(chunk)
        return b"".join(chunks)

    def close(self) -> None:
        self._buf.clear()


class _SocketWriter:
    def __init__(self, conn: _Any) -> None:
        self._conn = conn

    def write(self, data: bytes | bytearray | memoryview) -> int:
        payload = bytes(data)
        self._conn.sendall(payload)
        return len(payload)

    def flush(self) -> None:
        return None

    def close(self) -> None:
        return None


class _DatagramReader:
    def __init__(self, payload: bytes) -> None:
        self._buf = bytearray(payload)

    def read(self, size: int = -1) -> bytes:
        if size is None or size < 0:
            out = bytes(self._buf)
            self._buf.clear()
            return out
        out = bytes(self._buf[:size])
        del self._buf[:size]
        return out

    def readline(self, size: int = -1) -> bytes:
        if size == 0:
            return b""
        if size is not None and size > 0:
            return self.read(size)
        pos = self._buf.find(b"\n")
        if pos == -1:
            return self.read(-1)
        out = bytes(self._buf[: pos + 1])
        del self._buf[: pos + 1]
        return out

    def close(self) -> None:
        self._buf.clear()


class _DatagramWriter:
    def __init__(self) -> None:
        self._buf = bytearray()

    def write(self, data: bytes | bytearray | memoryview) -> int:
        payload = bytes(data)
        self._buf.extend(payload)
        return len(payload)

    def flush(self) -> None:
        return None

    def value(self) -> bytes:
        return bytes(self._buf)

    def close(self) -> None:
        self._buf.clear()


class BaseRequestHandler:
    def __init__(self, request: _Any, client_address: _Any, server: _Any) -> None:
        self.request = request
        self.client_address = client_address
        self.server = server
        self.setup()
        try:
            self.handle()
        finally:
            self.finish()

    def setup(self) -> None:
        return None

    def handle(self) -> None:
        return None

    def finish(self) -> None:
        return None


class StreamRequestHandler(BaseRequestHandler):
    rbufsize = -1
    wbufsize = 0
    timeout = None

    def setup(self) -> None:
        self.connection = self.request
        self.rfile = _SocketReader(self.connection)
        self.wfile = _SocketWriter(self.connection)

    def finish(self) -> None:
        try:
            if hasattr(self.wfile, "flush"):
                self.wfile.flush()
        except Exception:
            pass
        for stream_name in ("rfile", "wfile"):
            stream = getattr(self, stream_name, None)
            if stream is not None:
                try:
                    stream.close()
                except Exception:
                    pass


class DatagramRequestHandler(BaseRequestHandler):
    def setup(self) -> None:
        data, sock = self.request
        self.packet = bytes(data)
        self.socket = sock
        self.rfile = _DatagramReader(self.packet)
        self.wfile = _DatagramWriter()

    def finish(self) -> None:
        try:
            payload = self.wfile.value()
            if payload:
                self.socket.sendto(payload, self.client_address)
        except Exception:
            pass
        finally:
            self.rfile.close()
            self.wfile.close()


_NEXT_PORT = 49000
_SERVERS: dict[tuple[str, int], "BaseServer"] = {}


def _allocate_port() -> int:
    global _NEXT_PORT
    _NEXT_PORT += 1
    return _NEXT_PORT


def _lookup_server(host: str, port: int) -> "BaseServer" | None:
    return _SERVERS.get((host, int(port)))


class BaseServer:
    timeout = None

    def __init__(
        self,
        server_address: tuple[str, int],
        RequestHandlerClass: type[BaseRequestHandler],
    ) -> None:
        self.server_address = (str(server_address[0]), int(server_address[1]))
        self.RequestHandlerClass = RequestHandlerClass
        self._closed = False
        self._molt_shutdown_request = False

    def fileno(self) -> int:
        return -1

    def server_activate(self) -> None:
        return None

    def server_bind(self) -> None:
        return None

    def get_request(self) -> tuple[_Any, _Any, int]:
        raise NotImplementedError

    def verify_request(self, request: _Any, client_address: _Any) -> bool:
        del request, client_address
        return True

    def process_request(self, request: _Any, client_address: _Any) -> None:
        self.finish_request(request, client_address)

    def finish_request(self, request: _Any, client_address: _Any) -> None:
        self.RequestHandlerClass(request, client_address, self)

    def close_request(self, request: _Any) -> None:
        try:
            request.close()
        except Exception:
            pass

    def handle_error(self, request: _Any, client_address: _Any) -> None:
        del request, client_address
        return None

    def handle_request(self) -> None:
        _MOLT_SOCKETSERVER_HANDLE_REQUEST(self)

    def serve_forever(self, poll_interval: float = 0.5) -> None:
        _MOLT_SOCKETSERVER_SERVE_FOREVER(self, float(poll_interval))

    def shutdown(self) -> None:
        _MOLT_SOCKETSERVER_SHUTDOWN(self)

    def server_close(self) -> None:
        self._molt_shutdown_request = True
        self._closed = True
        _MOLT_SOCKETSERVER_UNREGISTER(self)
        if isinstance(self.server_address, tuple) and len(self.server_address) == 2:
            host, port = self.server_address
            _SERVERS.pop((str(host), int(port)), None)

    def __enter__(self):
        return self

    def __exit__(self, exc_type: _Any, exc: _Any, tb: _Any) -> None:
        self.server_close()


class TCPServer(BaseServer):
    address_family = socket.AF_INET
    socket_type = socket.SOCK_STREAM
    request_queue_size = 5
    allow_reuse_address = False

    def __init__(
        self,
        server_address: tuple[str, int],
        RequestHandlerClass: type[BaseRequestHandler],
    ) -> None:
        super().__init__(server_address, RequestHandlerClass)
        self.server_name = str(server_address[0])
        self.server_port = int(server_address[1])
        self.socket = socket.socket(self.address_family, self.socket_type)
        if self.allow_reuse_address:
            self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.server_bind()
        self.server_activate()
        # keep accept responsive so serve_forever can observe shutdown quickly
        self.socket.settimeout(0.05)
        _SERVERS[self.server_address] = self
        _MOLT_SOCKETSERVER_REGISTER(self)

    def _dispatch(self, request_bytes: bytes, timeout: float = 5.0) -> bytes:
        request_id = _MOLT_SOCKETSERVER_DISPATCH_BEGIN(self, bytes(request_bytes))
        deadline = _time_module.monotonic() + timeout
        while True:
            response = _MOLT_SOCKETSERVER_DISPATCH_POLL(self, request_id)
            if response is not None:
                return response
            if self._closed:
                _MOLT_SOCKETSERVER_DISPATCH_CANCEL(self, request_id)
                raise OSError("server closed")
            if _time_module.monotonic() >= deadline:
                _MOLT_SOCKETSERVER_DISPATCH_CANCEL(self, request_id)
                raise TimeoutError("server request timed out")
            _time_module.sleep(0.001)

    def server_bind(self) -> None:
        self.socket.bind(self.server_address)
        host, port = self.socket.getsockname()
        self.server_address = (str(host), int(port))
        self.server_name = str(host)
        self.server_port = int(port)

    def server_activate(self) -> None:
        self.socket.listen(int(self.request_queue_size))

    def get_request(self) -> tuple[_Any, _Any, int]:
        while True:
            pending = _MOLT_SOCKETSERVER_GET_REQUEST_POLL(self)
            if pending is not None:
                request_id, request_bytes = pending
                request = _FakeSocket(request_bytes)
                return request, ("127.0.0.1", 0), int(request_id)
            if self._closed or self._molt_shutdown_request:
                raise OSError("server closed")
            try:
                request, client_address = self.socket.accept()
            except TimeoutError:
                continue
            try:
                request.settimeout(None)
            except Exception:
                pass
            return request, client_address, -1

    def server_close(self) -> None:
        try:
            self.socket.close()
        except Exception:
            pass
        super().server_close()


class UDPServer(TCPServer):
    socket_type = socket.SOCK_DGRAM
    max_packet_size = 8192

    def server_activate(self) -> None:
        return None

    def get_request(self) -> tuple[_Any, _Any, int]:
        while True:
            if self._closed or self._molt_shutdown_request:
                raise OSError("server closed")
            try:
                data, client_address = self.socket.recvfrom(self.max_packet_size)
            except TimeoutError:
                continue
            return (bytes(data), self.socket), client_address, -1

    def close_request(self, request: _Any) -> None:
        del request
        return None


if hasattr(socket, "AF_UNIX"):

    class UnixStreamServer(TCPServer):
        address_family = socket.AF_UNIX

    class UnixDatagramServer(UDPServer):
        address_family = socket.AF_UNIX

else:

    class UnixStreamServer(TCPServer):
        pass

    class UnixDatagramServer(UDPServer):
        pass


class ThreadingMixIn:
    daemon_threads = False

    def process_request_thread(self, request: _Any, client_address: _Any) -> None:
        try:
            self.finish_request(request, client_address)
        except Exception:
            self.handle_error(request, client_address)
        finally:
            self.close_request(request)

    def process_request(self, request: _Any, client_address: _Any) -> None:
        worker = threading.Thread(
            target=self.process_request_thread,
            args=(request, client_address),
            daemon=bool(self.daemon_threads),
        )
        worker.start()


class ThreadingTCPServer(ThreadingMixIn, TCPServer):
    pass


class ThreadingUDPServer(ThreadingMixIn, UDPServer):
    pass


class ThreadingUnixStreamServer(ThreadingMixIn, UnixStreamServer):
    pass


class ThreadingUnixDatagramServer(ThreadingMixIn, UnixDatagramServer):
    pass


class ForkingMixIn:
    def process_request(self, request: _Any, client_address: _Any) -> None:
        # Keep semantics deterministic in Molt: fallback to in-process handling
        # while preserving the API surface.
        super().process_request(request, client_address)


class ForkingTCPServer(ForkingMixIn, TCPServer):
    pass


class ForkingUDPServer(ForkingMixIn, UDPServer):
    pass


class ForkingUnixStreamServer(ForkingMixIn, UnixStreamServer):
    pass


class ForkingUnixDatagramServer(ForkingMixIn, UnixDatagramServer):
    pass


__all__ = [
    "BaseRequestHandler",
    "BaseServer",
    "BufferedIOBase",
    "DatagramRequestHandler",
    "ForkingMixIn",
    "ForkingTCPServer",
    "ForkingUDPServer",
    "ForkingUnixDatagramServer",
    "ForkingUnixStreamServer",
    "StreamRequestHandler",
    "TCPServer",
    "ThreadingMixIn",
    "ThreadingTCPServer",
    "ThreadingUDPServer",
    "ThreadingUnixDatagramServer",
    "ThreadingUnixStreamServer",
    "UDPServer",
    "UnixDatagramServer",
    "UnixStreamServer",
    "os",
    "selectors",
    "socket",
    "sys",
    "threading",
    "time",
]

globals().pop("_require_intrinsic", None)
