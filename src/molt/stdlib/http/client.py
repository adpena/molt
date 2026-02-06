"""Minimal intrinsic-friendly http.client subset for Molt."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

import socket as _socket  # noqa: E402
import socketserver as _socketserver  # noqa: E402


class HTTPResponse:
    def __init__(
        self, status: int, stream: Any | None = None, body: bytes = b""
    ) -> None:
        self.status = status
        self._stream = stream
        self._body = body

    def read(self, amt: int | None = None) -> bytes:
        if self._stream is None:
            if amt is None or amt < 0:
                out = self._body
                self._body = b""
                return out
            out = self._body[:amt]
            self._body = self._body[amt:]
            return out
        if self._stream is None:
            return b""
        if amt is None or amt < 0:
            data = self._stream.read()
        else:
            data = self._stream.read(amt)
        if isinstance(data, str):
            return data.encode("iso-8859-1", "surrogateescape")
        if isinstance(data, (bytes, bytearray, memoryview)):
            return bytes(data)
        return b""


class HTTPConnection:
    def __init__(
        self, host: str, port: int | None = None, timeout: float | None = None
    ) -> None:
        self.host = host
        self.port = int(port) if port is not None else 80
        self.timeout = timeout
        self._sock: _socket.socket | None = None
        self._fake_server: Any | None = None
        self._fake_response: bytes | None = None
        self._response_stream: Any | None = None
        self._request_line = b""
        self._headers: list[tuple[str, str]] = []
        self._buffer: list[bytes] = []

    def connect(self) -> None:
        fake = _socketserver._lookup_server(self.host, self.port)
        if fake is not None:
            self._fake_server = fake
            self._sock = None
            return
        if self._sock is not None:
            return
        sock = _socket.socket(_socket.AF_INET, _socket.SOCK_STREAM)
        if self.timeout is not None:
            sock.settimeout(self.timeout)
        sock.connect((self.host, self.port))
        self._sock = sock

    def putrequest(
        self,
        method: str,
        url: str,
        skip_host: bool = False,
        skip_accept_encoding: bool = False,
    ) -> None:
        del skip_host, skip_accept_encoding
        self._request_line = f"{method} {url} HTTP/1.1\\r\\n".encode(
            "ascii", "surrogateescape"
        )
        self._headers = []
        self._buffer = [self._request_line]

    def putheader(self, header: str, *values: Any) -> None:
        if not values:
            value = ""
        elif len(values) == 1:
            value = str(values[0])
        else:
            value = ", ".join(str(v) for v in values)
        self._headers.append((str(header), value))

    def _request_bytes(self) -> bytes:
        lines = [self._request_line]
        has_host = any(name.lower() == "host" for name, _ in self._headers)
        if not has_host:
            host_value = self.host
            if self.port not in (80, 443):
                host_value = f"{host_value}:{self.port}"
            lines.append(f"Host: {host_value}\\r\\n".encode("ascii", "surrogateescape"))
        for name, value in self._headers:
            line = f"{name}: {value}\\r\\n".encode("ascii", "surrogateescape")
            lines.append(line)
        lines.append(b"\\r\\n")
        return b"".join(lines)

    def endheaders(self, message_body: bytes | None = None) -> None:
        if self._fake_server is not None:
            body = message_body or b""
            self._fake_response = self._fake_server._dispatch(
                self._request_bytes() + body, self.timeout or 5.0
            )
            return
        if self._sock is None:
            self.connect()
        if self._fake_server is not None:
            body = message_body or b""
            self._fake_response = self._fake_server._dispatch(
                self._request_bytes() + body, self.timeout or 5.0
            )
            return
        assert self._sock is not None
        payload = self._request_bytes()
        self._sock.sendall(payload)
        if message_body:
            self._sock.sendall(message_body)

    def request(
        self,
        method: str,
        url: str,
        body: bytes | None = None,
        headers: dict[str, Any] | None = None,
    ) -> None:
        self.putrequest(method, url, skip_accept_encoding=True)
        if headers:
            for name, value in headers.items():
                self.putheader(name, value)
        has_content_length = False
        for header_name, _ in self._headers:
            if str(header_name).lower() == "content-length":
                has_content_length = True
                break
        if body is not None and not has_content_length:
            self.putheader("Content-Length", str(len(body)))
        self.endheaders(body)

    def getresponse(self) -> HTTPResponse:
        if self._fake_response is not None:
            raw = self._fake_response
            self._fake_response = None
            head, _, body = raw.partition(b"\r\n\r\n")
            first = head.split(b"\r\n", 1)[0]
            text = first.decode("iso-8859-1", "surrogateescape").strip()
            parts = text.split(None, 2)
            if len(parts) >= 2:
                try:
                    status = int(parts[1])
                except Exception:
                    status = 0
            else:
                status = 0
            return HTTPResponse(status, body=body)
        if self._sock is None:
            raise OSError("not connected")
        stream = self._sock.makefile("rb")
        self._response_stream = stream
        status_line = stream.readline(65537)
        text = status_line.decode("iso-8859-1", "surrogateescape").strip()
        parts = text.split(None, 2)
        if len(parts) >= 2:
            try:
                status = int(parts[1])
            except Exception:
                status = 0
        else:
            status = 0
        while True:
            line = stream.readline(65537)
            if not line or line in (b"\r\n", b"\n"):
                break
        return HTTPResponse(status, stream)

    def close(self) -> None:
        stream = self._response_stream
        if stream is not None:
            try:
                stream.close()
            except Exception:
                pass
            self._response_stream = None
        self._fake_response = None
        self._fake_server = None
        sock = self._sock
        if sock is not None:
            try:
                sock.close()
            except Exception:
                pass
            self._sock = None


__all__ = ["HTTPConnection", "HTTPResponse"]
