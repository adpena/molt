"""Intrinsic-backed http.client surface for Molt."""

from __future__ import annotations

from abc import ABCMeta as _ABCMeta
from functools import lru_cache as _lru_cache
import importlib as _importlib
import sys
from types import ModuleType as _ModuleType
from typing import Any as _Any

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

_MOLT_HTTP_EXECUTE = _require_intrinsic("molt_http_client_execute", globals())
_MOLT_HTTP_CONN_NEW = _require_intrinsic("molt_http_client_connection_new", globals())
_MOLT_HTTP_CONN_PUTREQUEST = _require_intrinsic(
    "molt_http_client_connection_putrequest", globals()
)
_MOLT_HTTP_CONN_PUTHEADER = _require_intrinsic(
    "molt_http_client_connection_putheader", globals()
)
_MOLT_HTTP_CONN_ENDHEADERS = _require_intrinsic(
    "molt_http_client_connection_endheaders", globals()
)
_MOLT_HTTP_CONN_SEND = _require_intrinsic("molt_http_client_connection_send", globals())
_MOLT_HTTP_CONN_REQUEST = _require_intrinsic(
    "molt_http_client_connection_request", globals()
)
_MOLT_HTTP_CONN_GETRESPONSE = _require_intrinsic(
    "molt_http_client_connection_getresponse", globals()
)
_MOLT_HTTP_CONN_CLOSE = _require_intrinsic(
    "molt_http_client_connection_close", globals()
)
_MOLT_HTTP_CONN_DROP = _require_intrinsic("molt_http_client_connection_drop", globals())
_MOLT_HTTP_CONN_GET_BUFFER = _require_intrinsic(
    "molt_http_client_connection_get_buffer", globals()
)
_MOLT_HTTP_MESSAGE_NEW = _require_intrinsic("molt_http_message_new", globals())
_MOLT_HTTP_MESSAGE_PARSE = _require_intrinsic("molt_http_message_parse", globals())
_MOLT_HTTP_MESSAGE_SET_RAW = _require_intrinsic("molt_http_message_set_raw", globals())
_MOLT_HTTP_MESSAGE_GET = _require_intrinsic("molt_http_message_get", globals())
_MOLT_HTTP_MESSAGE_GET_ALL = _require_intrinsic("molt_http_message_get_all", globals())
_MOLT_HTTP_MESSAGE_ITEMS = _require_intrinsic("molt_http_message_items", globals())
_MOLT_HTTP_MESSAGE_CONTAINS = _require_intrinsic(
    "molt_http_message_contains", globals()
)
_MOLT_HTTP_MESSAGE_LEN = _require_intrinsic("molt_http_message_len", globals())
_MOLT_HTTP_MESSAGE_DROP = _require_intrinsic("molt_http_message_drop", globals())
_MOLT_HTTP_RESP_READ = _require_intrinsic("molt_http_client_response_read", globals())
_MOLT_HTTP_RESP_CLOSE = _require_intrinsic("molt_http_client_response_close", globals())
_MOLT_HTTP_RESP_DROP = _require_intrinsic("molt_http_client_response_drop", globals())
_MOLT_HTTP_RESP_STATUS = _require_intrinsic(
    "molt_http_client_response_getstatus", globals()
)
_MOLT_HTTP_RESP_REASON = _require_intrinsic(
    "molt_http_client_response_getreason", globals()
)
_MOLT_HTTP_RESP_MESSAGE = _require_intrinsic(
    "molt_http_client_response_message",
    globals(),
)
_MOLT_HTTP_STATUS_CONSTANTS = _require_intrinsic(
    "molt_http_status_constants", globals()
)
_MOLT_HTTP_STATUS_RESPONSES = _require_intrinsic(
    "molt_http_status_responses", globals()
)
_MOLT_HTTP_CLIENT_URLSPLIT = _require_intrinsic("molt_http_client_urlsplit", globals())


def _safe_import(module_name: str) -> _ModuleType:
    try:
        return _importlib.import_module(module_name)
    except Exception:
        missing = _ModuleType(module_name)

        def _missing_attr(name: str, *, _module: str = module_name):
            raise RuntimeError(f"{_module}.{name} is unavailable in this runtime")

        missing.__getattr__ = _missing_attr  # type: ignore[attr-defined]
        return missing


collections = _safe_import("collections")
email = _safe_import("email")
errno = _safe_import("errno")
http = _safe_import("http")
io = _safe_import("io")
re = _safe_import("re")
socket = _safe_import("socket")
ssl = _safe_import("ssl")

_HTTPStatus = getattr(http, "HTTPStatus", None)
if _HTTPStatus is None:
    raise RuntimeError("http.HTTPStatus is unavailable")


def _load_status_constants() -> dict[str, int]:
    payload = _MOLT_HTTP_STATUS_CONSTANTS()
    if not isinstance(payload, dict):
        raise RuntimeError("http status constants intrinsic returned invalid payload")
    out: dict[str, int] = {}
    for key, value in payload.items():
        out[str(key)] = int(value)
    return out


_HTTP_STATUS_CONSTANTS = _load_status_constants()


def _load_status_responses() -> dict[int, str]:
    payload = _MOLT_HTTP_STATUS_RESPONSES()
    if not isinstance(payload, dict):
        raise RuntimeError("http status responses intrinsic returned invalid payload")
    out: dict[int, str] = {}
    for key, value in payload.items():
        out[int(key)] = str(value)
    return out


class _SplitResult(tuple):
    __slots__ = ()
    _fields = ("scheme", "netloc", "path", "query", "fragment")

    def __new__(
        cls,
        scheme: str,
        netloc: str,
        path: str,
        query: str,
        fragment: str,
    ):
        return tuple.__new__(cls, (scheme, netloc, path, query, fragment))

    @property
    def scheme(self) -> str:
        return self[0]

    @property
    def netloc(self) -> str:
        return self[1]

    @property
    def path(self) -> str:
        return self[2]

    @property
    def query(self) -> str:
        return self[3]

    @property
    def fragment(self) -> str:
        return self[4]


def _http_client_urlsplit(
    url: str,
    scheme: str = "",
    allow_fragments: bool = True,
) -> _SplitResult:
    out = _MOLT_HTTP_CLIENT_URLSPLIT(str(url), str(scheme), bool(allow_fragments))
    if (
        not isinstance(out, tuple)
        or len(out) != 5
        or not all(isinstance(item, str) for item in out)
    ):
        raise RuntimeError("http.client urlsplit intrinsic returned invalid value")
    return _SplitResult(*out)


# Match CPython's exposed wrapper type (`_lru_cache_wrapper`).
urlsplit = _lru_cache(maxsize=128)(_http_client_urlsplit)

HTTP_PORT = 80
HTTPS_PORT = 443


class HTTPException(Exception):
    pass


error = HTTPException


class NotConnected(HTTPException):
    pass


class InvalidURL(HTTPException):
    pass


class UnknownProtocol(HTTPException):
    pass


class UnknownTransferEncoding(HTTPException):
    pass


class UnimplementedFileMode(HTTPException):
    pass


class ImproperConnectionState(HTTPException):
    pass


class CannotSendRequest(ImproperConnectionState):
    pass


class CannotSendHeader(ImproperConnectionState):
    pass


class ResponseNotReady(ImproperConnectionState):
    pass


class BadStatusLine(HTTPException):
    def __init__(self, line: str = "") -> None:
        super().__init__(line)
        self.line = line


class LineTooLong(HTTPException):
    pass


class RemoteDisconnected(ConnectionResetError, BadStatusLine):
    def __init__(self, *args: _Any) -> None:
        if not args:
            args = ("Remote end closed connection without response",)
        super().__init__(*args)


class IncompleteRead(HTTPException):
    def __init__(self, partial: bytes, expected: int | None = None) -> None:
        self.partial = bytes(partial)
        self.expected = expected
        super().__init__(self.partial, self.expected)


class HTTPMessage:
    __slots__ = ("_handle",)

    def __init__(self, _handle: int | None = None) -> None:
        handle = _MOLT_HTTP_MESSAGE_NEW() if _handle is None else _handle
        self._handle = int(handle)

    @classmethod
    def _from_handle(cls, handle: int) -> "HTTPMessage":
        msg = cls.__new__(cls)
        msg._handle = int(handle)
        return msg

    def set_raw(self, name: str, value: str) -> None:
        _MOLT_HTTP_MESSAGE_SET_RAW(self._handle, str(name), str(value))

    def add_header(self, name: str, value: str, **_params: _Any) -> None:
        del _params
        self.set_raw(name, value)

    def get(self, name: str, failobj: _Any = None) -> _Any:
        return _MOLT_HTTP_MESSAGE_GET(self._handle, str(name), failobj)

    def get_all(self, name: str, failobj: _Any = None):
        out = _MOLT_HTTP_MESSAGE_GET_ALL(self._handle, str(name))
        if not isinstance(out, list):
            raise RuntimeError(
                "http.client message get_all intrinsic returned invalid value"
            )
        values = [str(value) for value in out]
        return values if values else failobj

    def items(self) -> list[tuple[str, str]]:
        out = _MOLT_HTTP_MESSAGE_ITEMS(self._handle)
        if not isinstance(out, list):
            raise RuntimeError(
                "http.client message items intrinsic returned invalid value"
            )
        pairs: list[tuple[str, str]] = []
        for item in out:
            if not (isinstance(item, tuple) and len(item) >= 2):
                raise RuntimeError(
                    "http.client message items intrinsic returned invalid header pair"
                )
            pairs.append((str(item[0]), str(item[1])))
        return pairs

    def keys(self) -> list[str]:
        return [key for key, _ in self.items()]

    def values(self) -> list[str]:
        return [value for _, value in self.items()]

    def __contains__(self, name: object) -> bool:
        if not isinstance(name, str):
            return False
        return bool(_MOLT_HTTP_MESSAGE_CONTAINS(self._handle, name))

    def __iter__(self):
        for key, _ in self.items():
            yield key

    def __len__(self) -> int:
        return int(_MOLT_HTTP_MESSAGE_LEN(self._handle))

    def __getitem__(self, name: str) -> str:
        value = self.get(name, None)
        if value is None:
            raise KeyError(name)
        return str(value)

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is None:
            return
        try:
            _MOLT_HTTP_MESSAGE_DROP(handle)
        except Exception:
            pass
        self._handle = None


def parse_headers(fp, _class=HTTPMessage):
    lines: list[bytes] = []
    while True:
        line = fp.readline()
        if not line:
            break
        if isinstance(line, str):
            line = line.encode("iso-8859-1", "replace")
        lines.append(bytes(line))
        if line in (b"\r\n", b"\n"):
            break
    handle = int(_MOLT_HTTP_MESSAGE_PARSE(b"".join(lines)))
    if isinstance(_class, type) and issubclass(_class, HTTPMessage):
        return _class._from_handle(handle)
    try:
        message = _class()
    except Exception as exc:
        raise RuntimeError("http.client header class construction failed") from exc
    try:
        payload = _MOLT_HTTP_MESSAGE_ITEMS(handle)
        if not isinstance(payload, list):
            raise RuntimeError(
                "http.client parse header intrinsic returned invalid value"
            )
        set_raw = getattr(message, "set_raw", None)
        add_header = getattr(message, "add_header", None)
        for item in payload:
            if not (isinstance(item, tuple) and len(item) >= 2):
                raise RuntimeError(
                    "http.client parse header intrinsic returned invalid header pair"
                )
            name = str(item[0])
            value = str(item[1])
            if callable(set_raw):
                set_raw(name, value)
            elif callable(add_header):
                add_header(name, value)
            else:
                headers = getattr(message, "_headers", None)
                if isinstance(headers, list):
                    headers.append((name, value))
                else:
                    raise RuntimeError(
                        "http.client header object does not support insertion"
                    )
        return message
    finally:
        _MOLT_HTTP_MESSAGE_DROP(handle)


class HTTPResponse(metaclass=_ABCMeta):
    _handle: int
    closed: bool
    _msg: HTTPMessage | None

    def __init__(self, handle: int) -> None:
        self._handle = int(handle)
        self.closed = False
        self._msg = None

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
        self._msg = None
        self._handle = None

    def getheader(self, name: str, default: _Any = None) -> _Any:
        values = self.msg.get_all(str(name))
        if not values:
            return default
        if len(values) == 1:
            return values[0]
        return ", ".join(values)

    def getheaders(self) -> list[tuple[str, str]]:
        return self.msg.items()

    @property
    def status(self) -> int:
        return int(_MOLT_HTTP_RESP_STATUS(self._handle))

    @property
    def reason(self) -> str:
        return str(_MOLT_HTTP_RESP_REASON(self._handle))

    @property
    def msg(self) -> HTTPMessage:
        cached = self._msg
        if cached is not None:
            return cached
        handle = int(_MOLT_HTTP_RESP_MESSAGE(self._handle))
        out = HTTPMessage._from_handle(handle)
        self._msg = out
        return out


def _coerce_connection_buffer(payload: _Any, context: str) -> list[bytes]:
    if not isinstance(payload, list):
        raise RuntimeError(
            f"http.client {context} intrinsic returned invalid buffer payload"
        )
    return [bytes(part) for part in payload]


class HTTPConnection:
    host: str
    port: int
    timeout: float | None
    _conn_handle: int
    _response: HTTPResponse | None

    def __init__(
        self, host: str, port: int | None = None, timeout: float | None = None
    ) -> None:
        self.host = str(host)
        self.port = int(port) if port is not None else HTTP_PORT
        self.timeout = None if timeout is None else float(timeout)
        self._conn_handle = int(_MOLT_HTTP_CONN_NEW(self.host, self.port, self.timeout))
        self._response = None

    @property
    def _buffer(self) -> list[bytes]:
        return _coerce_connection_buffer(
            _MOLT_HTTP_CONN_GET_BUFFER(self._conn_handle),
            "connection_get_buffer",
        )

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
        _coerce_connection_buffer(
            _MOLT_HTTP_CONN_PUTREQUEST(
                self._conn_handle,
                str(method),
                str(url),
                bool(skip_host),
                bool(skip_accept_encoding),
            ),
            "putrequest",
        )

    def putheader(self, header: str, *values: _Any) -> None:
        header_text = str(header)
        if not values:
            value = ""
        elif len(values) == 1:
            value = str(values[0])
        else:
            value = ", ".join(str(v) for v in values)
        _coerce_connection_buffer(
            _MOLT_HTTP_CONN_PUTHEADER(self._conn_handle, header_text, value),
            "putheader",
        )

    def endheaders(
        self, message_body: bytes | bytearray | memoryview | None = None
    ) -> None:
        _coerce_connection_buffer(
            _MOLT_HTTP_CONN_ENDHEADERS(
                self._conn_handle,
                None if message_body is None else bytes(message_body),
            ),
            "endheaders",
        )

    def send(self, data: bytes | bytearray | memoryview) -> None:
        _coerce_connection_buffer(
            _MOLT_HTTP_CONN_SEND(self._conn_handle, bytes(data)), "send"
        )

    def request(
        self,
        method: str,
        url: str,
        body: bytes | bytearray | memoryview | None = None,
        headers: dict[str, _Any] | None = None,
    ) -> None:
        header_map = {} if headers is None else dict(headers)
        _coerce_connection_buffer(
            _MOLT_HTTP_CONN_REQUEST(
                self._conn_handle,
                str(method),
                str(url),
                None if body is None else bytes(body),
                header_map,
            ),
            "request",
        )

    def getresponse(self) -> HTTPResponse:
        if self._response is not None and not self._response.closed:
            self._response.close()
        self._response = HTTPResponse(
            int(_MOLT_HTTP_CONN_GETRESPONSE(self._conn_handle))
        )
        return self._response

    def close(self) -> None:
        response = self._response
        if response is not None:
            try:
                response.close()
            except Exception:
                pass
            self._response = None
        _MOLT_HTTP_CONN_CLOSE(self._conn_handle)

    def __del__(self) -> None:
        handle = getattr(self, "_conn_handle", None)
        if handle is None:
            return
        try:
            _MOLT_HTTP_CONN_DROP(handle)
        except Exception:
            pass
        self._conn_handle = None


class HTTPSConnection(HTTPConnection):
    def __init__(
        self, host: str, port: int | None = None, timeout: float | None = None
    ) -> None:
        super().__init__(host, HTTPS_PORT if port is None else port, timeout)


responses = _load_status_responses()


def _export_status_constant(name: str, code: int) -> None:
    member = getattr(_HTTPStatus, name, None)
    if member is None:
        member = _HTTPStatus(code)
    globals()[name] = member


for _name, _code in _HTTP_STATUS_CONSTANTS.items():
    _export_status_constant(str(_name), int(_code))


__all__ = [
    "ACCEPTED",
    "ALREADY_REPORTED",
    "BAD_GATEWAY",
    "BAD_REQUEST",
    "BadStatusLine",
    "CONFLICT",
    "CONTENT_TOO_LARGE",
    "CONTINUE",
    "CREATED",
    "CannotSendHeader",
    "CannotSendRequest",
    "EARLY_HINTS",
    "EXPECTATION_FAILED",
    "FAILED_DEPENDENCY",
    "FORBIDDEN",
    "FOUND",
    "GATEWAY_TIMEOUT",
    "GONE",
    "HTTPConnection",
    "HTTPException",
    "HTTPMessage",
    "HTTPResponse",
    "HTTPSConnection",
    "HTTPS_PORT",
    "HTTP_PORT",
    "HTTP_VERSION_NOT_SUPPORTED",
    "IM_A_TEAPOT",
    "IM_USED",
    "INSUFFICIENT_STORAGE",
    "INTERNAL_SERVER_ERROR",
    "ImproperConnectionState",
    "IncompleteRead",
    "InvalidURL",
    "LENGTH_REQUIRED",
    "LOCKED",
    "LOOP_DETECTED",
    "LineTooLong",
    "METHOD_NOT_ALLOWED",
    "MISDIRECTED_REQUEST",
    "MOVED_PERMANENTLY",
    "MULTIPLE_CHOICES",
    "MULTI_STATUS",
    "NETWORK_AUTHENTICATION_REQUIRED",
    "NON_AUTHORITATIVE_INFORMATION",
    "NOT_ACCEPTABLE",
    "NOT_EXTENDED",
    "NOT_FOUND",
    "NOT_IMPLEMENTED",
    "NOT_MODIFIED",
    "NO_CONTENT",
    "NotConnected",
    "OK",
    "PARTIAL_CONTENT",
    "PAYMENT_REQUIRED",
    "PERMANENT_REDIRECT",
    "PRECONDITION_FAILED",
    "PRECONDITION_REQUIRED",
    "PROCESSING",
    "PROXY_AUTHENTICATION_REQUIRED",
    "RANGE_NOT_SATISFIABLE",
    "REQUESTED_RANGE_NOT_SATISFIABLE",
    "REQUEST_ENTITY_TOO_LARGE",
    "REQUEST_HEADER_FIELDS_TOO_LARGE",
    "REQUEST_TIMEOUT",
    "REQUEST_URI_TOO_LONG",
    "RESET_CONTENT",
    "RemoteDisconnected",
    "ResponseNotReady",
    "SEE_OTHER",
    "SERVICE_UNAVAILABLE",
    "SWITCHING_PROTOCOLS",
    "TEMPORARY_REDIRECT",
    "TOO_EARLY",
    "TOO_MANY_REQUESTS",
    "UNAUTHORIZED",
    "UNAVAILABLE_FOR_LEGAL_REASONS",
    "UNPROCESSABLE_CONTENT",
    "UNPROCESSABLE_ENTITY",
    "UNSUPPORTED_MEDIA_TYPE",
    "UPGRADE_REQUIRED",
    "URI_TOO_LONG",
    "USE_PROXY",
    "UnimplementedFileMode",
    "UnknownProtocol",
    "UnknownTransferEncoding",
    "VARIANT_ALSO_NEGOTIATES",
    "collections",
    "email",
    "errno",
    "error",
    "http",
    "io",
    "parse_headers",
    "re",
    "responses",
    "socket",
    "ssl",
    "sys",
    "urlsplit",
]
