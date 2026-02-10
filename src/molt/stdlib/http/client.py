"""Intrinsic-backed http.client subset for Molt."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

_MOLT_HTTP_EXECUTE = _require_intrinsic("molt_http_client_execute", globals())
_MOLT_HTTP_RESP_READ = _require_intrinsic("molt_http_client_response_read", globals())
_MOLT_HTTP_RESP_CLOSE = _require_intrinsic("molt_http_client_response_close", globals())
_MOLT_HTTP_RESP_DROP = _require_intrinsic("molt_http_client_response_drop", globals())
_MOLT_HTTP_RESP_STATUS = _require_intrinsic(
    "molt_http_client_response_getstatus", globals()
)
_MOLT_HTTP_RESP_REASON = _require_intrinsic(
    "molt_http_client_response_getreason", globals()
)
_MOLT_HTTP_RESP_GETHEADER = _require_intrinsic(
    "molt_http_client_response_getheader",
    globals(),
)
_MOLT_HTTP_RESP_GETHEADERS = _require_intrinsic(
    "molt_http_client_response_getheaders",
    globals(),
)


class HTTPResponse:
    _handle: int
    closed: bool

    def __init__(self, handle: int) -> None:
        self._handle = int(handle)
        self.closed = False

    def read(self, amt: int | None = None) -> bytes:
        if self.closed:
            raise ValueError("I/O operation on closed file.")
        size = -1 if amt is None else int(amt)
        return _MOLT_HTTP_RESP_READ(self._handle, size)

    def close(self) -> None:
        if not self.closed:
            _MOLT_HTTP_RESP_CLOSE(self._handle)
            self.closed = True

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is None:
            return
        try:
            _MOLT_HTTP_RESP_DROP(handle)
        except Exception:
            pass
        self._handle = None

    def getheader(self, name: str, default: Any = None) -> Any:
        return _MOLT_HTTP_RESP_GETHEADER(self._handle, str(name), default)

    def getheaders(self) -> list[tuple[str, str]]:
        out = _MOLT_HTTP_RESP_GETHEADERS(self._handle)
        if not isinstance(out, list):
            raise RuntimeError(
                "http.client response headers intrinsic returned invalid value"
            )
        return [(str(k), str(v)) for (k, v) in out]

    @property
    def status(self) -> int:
        return int(_MOLT_HTTP_RESP_STATUS(self._handle))

    @property
    def reason(self) -> str:
        return str(_MOLT_HTTP_RESP_REASON(self._handle))


class HTTPConnection:
    host: str
    port: int
    timeout: float | None
    _method: str | None
    _url: str | None
    _headers: list[tuple[str, str]]
    _body: bytearray
    _buffer: list[bytes]
    _skip_host: bool
    _skip_accept_encoding: bool
    _response: HTTPResponse | None

    def __init__(
        self, host: str, port: int | None = None, timeout: float | None = None
    ) -> None:
        self.host = str(host)
        self.port = int(port) if port is not None else 80
        self.timeout = None if timeout is None else float(timeout)
        self._method: str | None = None
        self._url: str | None = None
        self._headers: list[tuple[str, str]] = []
        self._body = bytearray()
        self._buffer: list[bytes] = []
        self._skip_host = False
        self._skip_accept_encoding = False
        self._response: HTTPResponse | None = None

    def connect(self) -> None:
        # Connection setup is deferred to request execution in the intrinsic.
        return None

    def putrequest(
        self,
        method: str,
        url: str,
        skip_host: bool = False,
        skip_accept_encoding: bool = False,
    ) -> None:
        method_text = str(method)
        url_text = str(url)
        self._method = method_text
        self._url = url_text
        self._headers = []
        self._body = bytearray()
        self._skip_host = bool(skip_host)
        self._skip_accept_encoding = bool(skip_accept_encoding)
        self._buffer = []
        self._buffer.append(f"{method_text} {url_text} HTTP/1.1\r\n".encode("ascii"))

    def putheader(self, header: str, *values: Any) -> None:
        header_text = str(header)
        if not values:
            value = ""
        elif len(values) == 1:
            value = str(values[0])
        else:
            value = ", ".join(str(v) for v in values)
        self._headers.append((header_text, value))

    def endheaders(
        self, message_body: bytes | bytearray | memoryview | None = None
    ) -> None:
        if (
            getattr(self, "_method", None) is None
            or getattr(self, "_url", None) is None
        ):
            raise OSError("request not started")
        if self._buffer and self._buffer[-1] != b"\r\n":
            if not self._skip_host and not any(
                name.lower() == "host" for name, _ in self._headers
            ):
                host_value = self.host
                if self.port != 80:
                    host_value = f"{host_value}:{self.port}"
                self._headers.insert(0, ("Host", host_value))
            if not self._skip_accept_encoding and not any(
                name.lower() == "accept-encoding" for name, _ in self._headers
            ):
                self._headers.append(("Accept-Encoding", "identity"))
            for name, value in self._headers:
                self._buffer.append(f"{name}: {value}\r\n".encode("ascii"))
            self._buffer.append(b"\r\n")
        if message_body is not None:
            body = bytes(message_body)
            self._body.extend(body)
            self._buffer.append(body)

    def send(self, data: bytes | bytearray | memoryview) -> None:
        if (
            getattr(self, "_method", None) is None
            or getattr(self, "_url", None) is None
        ):
            raise OSError("request not started")
        out = bytes(data)
        self._body.extend(out)
        self._buffer.append(out)

    def request(
        self,
        method: str,
        url: str,
        body: bytes | bytearray | memoryview | None = None,
        headers: dict[str, Any] | None = None,
    ) -> None:
        self.putrequest(method, url, skip_accept_encoding=True)
        if headers:
            for name, value in headers.items():
                self.putheader(name, value)
        if body is not None and not any(
            name.lower() == "content-length" for name, _ in self._headers
        ):
            self.putheader("Content-Length", str(len(body)))
        if body is not None:
            self._body.extend(bytes(body))

    def getresponse(self) -> HTTPResponse:
        if (
            getattr(self, "_method", None) is None
            or getattr(self, "_url", None) is None
        ):
            raise OSError("no request pending")
        if not self._skip_host and not any(
            name.lower() == "host" for name, _ in self._headers
        ):
            host_value = self.host
            if self.port != 80:
                host_value = f"{host_value}:{self.port}"
            self._headers.insert(0, ("Host", host_value))
        if not self._skip_accept_encoding and not any(
            name.lower() == "accept-encoding" for name, _ in self._headers
        ):
            self._headers.append(("Accept-Encoding", "identity"))
        if self._response is not None and not self._response.closed:
            self._response.close()
        handle = _MOLT_HTTP_EXECUTE(
            self.host,
            self.port,
            self.timeout,
            self._method,
            self._url,
            list(self._headers),
            bytes(self._body),
        )
        self._method = None
        self._url = None
        self._headers = []
        self._body = bytearray()
        self._buffer = []
        self._skip_host = False
        self._skip_accept_encoding = False
        self._response = HTTPResponse(int(handle))
        return self._response

    def close(self) -> None:
        response = self._response
        if response is not None:
            try:
                response.close()
            except Exception:
                pass
            self._response = None
        self._method = None
        self._url = None
        self._headers = []
        self._body = bytearray()
        self._buffer = []
        self._skip_host = False
        self._skip_accept_encoding = False


__all__ = ["HTTPConnection", "HTTPResponse"]
