"""Minimal socketserver subset for Molt (in-memory transport)."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any
import time

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())


@dataclass
class _PendingRequest:
    request_bytes: bytes
    response_bytes: bytes | None = None


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
    def __init__(self, conn: Any) -> None:
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
    def __init__(self, conn: Any) -> None:
        self._conn = conn

    def write(self, data: bytes | bytearray | memoryview) -> int:
        payload = bytes(data)
        self._conn.sendall(payload)
        return len(payload)

    def flush(self) -> None:
        return None

    def close(self) -> None:
        return None


class BaseRequestHandler:
    def __init__(self, request: Any, client_address: Any, server: Any) -> None:
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


_NEXT_PORT = 49000
_SERVERS: dict[tuple[str, int], "TCPServer"] = {}


def _allocate_port() -> int:
    global _NEXT_PORT
    _NEXT_PORT += 1
    return _NEXT_PORT


def _lookup_server(host: str, port: int) -> "TCPServer" | None:
    return _SERVERS.get((host, int(port)))


class TCPServer:
    allow_reuse_address = False

    def __init__(
        self, server_address: tuple[str, int], RequestHandlerClass: Any
    ) -> None:
        host, port = server_address
        if int(port) == 0:
            port = _allocate_port()
        self.server_address = (host, int(port))
        self.RequestHandlerClass = RequestHandlerClass
        self._closed = False
        self._pending: _PendingRequest | None = None
        _SERVERS[self.server_address] = self

    def fileno(self) -> int:
        return -1

    def _dispatch(self, request_bytes: bytes, timeout: float = 5.0) -> bytes:
        pending = _PendingRequest(request_bytes=request_bytes)
        self._pending = pending
        deadline = time.monotonic() + timeout
        while pending.response_bytes is None:
            if self._closed:
                raise OSError("server closed")
            if time.monotonic() >= deadline:
                raise TimeoutError("server request timed out")
            time.sleep(0.001)
        return pending.response_bytes

    def get_request(self) -> tuple[Any, Any, _PendingRequest]:
        deadline = time.monotonic() + 10.0
        while self._pending is None:
            if self._closed:
                raise OSError("server closed")
            if time.monotonic() >= deadline:
                raise TimeoutError("handle_request timed out")
            time.sleep(0.001)
        pending = self._pending
        self._pending = None
        request = _FakeSocket(pending.request_bytes)
        return request, ("127.0.0.1", 0), pending

    def process_request(self, request: Any, client_address: Any) -> None:
        self.finish_request(request, client_address)

    def finish_request(self, request: Any, client_address: Any) -> None:
        self.RequestHandlerClass(request, client_address, self)

    def close_request(self, request: Any) -> None:
        try:
            request.close()
        except Exception:
            pass

    def handle_request(self) -> None:
        request, client_address, pending = self.get_request()
        try:
            self.process_request(request, client_address)
        finally:
            response = request.response_bytes()
            self.close_request(request)
            if pending.response_bytes is None:
                pending.response_bytes = response

    def server_close(self) -> None:
        self._closed = True
        _SERVERS.pop(self.server_address, None)

    def __enter__(self) -> "TCPServer":
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.server_close()


__all__ = ["BaseRequestHandler", "StreamRequestHandler", "TCPServer"]
