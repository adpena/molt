"""Intrinsic-backed http.server surface for Molt."""

from __future__ import annotations

import importlib as _importlib
import socketserver
import sys
from types import ModuleType as _ModuleType

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_STDLIB_PROBE = _require_intrinsic("molt_stdlib_probe")
_MOLT_HTTP_PARSE_HEADER_PAIRS = _require_intrinsic(
    "molt_http_parse_header_pairs"
)
_MOLT_HTTP_SERVER_READ_REQUEST = _require_intrinsic(
    "molt_http_server_read_request"
)
_MOLT_HTTP_SERVER_COMPUTE_CLOSE_CONNECTION = _require_intrinsic(
    "molt_http_server_compute_close_connection"
)
_MOLT_HTTP_SERVER_HANDLE_ONE_REQUEST = _require_intrinsic(
    "molt_http_server_handle_one_request"
)
_MOLT_HTTP_SERVER_SEND_RESPONSE = _require_intrinsic(
    "molt_http_server_send_response"
)
_MOLT_HTTP_SERVER_SEND_RESPONSE_ONLY = _require_intrinsic(
    "molt_http_server_send_response_only"
)
_MOLT_HTTP_SERVER_SEND_HEADER = _require_intrinsic(
    "molt_http_server_send_header"
)
_MOLT_HTTP_SERVER_END_HEADERS = _require_intrinsic("molt_http_server_end_headers")
_MOLT_HTTP_SERVER_SEND_ERROR = _require_intrinsic("molt_http_server_send_error")
_MOLT_HTTP_SERVER_VERSION_STRING = _require_intrinsic(
    "molt_http_server_version_string"
)
_MOLT_HTTP_SERVER_DATE_TIME_STRING = _require_intrinsic(
    "molt_http_server_date_time_string"
)


def _safe_import(module_name: str) -> _ModuleType:
    try:
        return _importlib.import_module(module_name)
    except Exception:
        missing = _ModuleType(module_name)

        def _missing_attr(name: str, *, _module: str = module_name):
            raise RuntimeError(f"{_module}.{name} is unavailable in this runtime")

        missing.__getattr__ = _missing_attr  # type: ignore[attr-defined]
        return missing


copy = _safe_import("copy")
datetime = _safe_import("datetime")
email = _safe_import("email")
html = _safe_import("html")
http = _safe_import("http")
io = _safe_import("io")
itertools = _safe_import("itertools")
mimetypes = _safe_import("mimetypes")
os = _safe_import("os")
posixpath = _safe_import("posixpath")
select = _safe_import("select")
shutil = _safe_import("shutil")
socket = _safe_import("socket")
time = _safe_import("time")
urllib = _safe_import("urllib")

HTTPStatus = getattr(http, "HTTPStatus", None)
if HTTPStatus is None:
    raise RuntimeError("http.HTTPStatus is unavailable")

DEFAULT_ERROR_MESSAGE = """\
<!DOCTYPE HTML>
<html lang="en">
    <head>
        <meta charset="utf-8">
        <title>Error response</title>
    </head>
    <body>
        <h1>Error response</h1>
        <p>Error code: %(code)d</p>
        <p>Message: %(message)s.</p>
        <p>Error code explanation: %(code)s - %(explain)s.</p>
    </body>
</html>
"""
DEFAULT_ERROR_CONTENT_TYPE = "text/html;charset=utf-8"


def parse_headers(data: bytes) -> "_MoltHeaders":
    """Parse raw HTTP header bytes into a _MoltHeaders object.

    *data* should be the raw byte content of the headers section (everything
    after the request/status line up to and including the blank line that ends
    the header block).  Returns a ``_MoltHeaders`` instance backed by the
    Rust intrinsic parser.
    """
    if not isinstance(data, (bytes, bytearray)):
        raise TypeError(
            f"parse_headers: expected bytes or bytearray, got {type(data).__name__}"
        )
    pairs = _MOLT_HTTP_PARSE_HEADER_PAIRS(bytes(data))
    return _MoltHeaders(pairs)


class _MoltHeaders:
    __slots__ = ("_pairs",)

    def __init__(self, pairs: object) -> None:
        out: list[tuple[str, str]] = []
        if isinstance(pairs, list):
            for item in pairs:
                if (
                    isinstance(item, tuple)
                    and len(item) >= 2
                    and isinstance(item[0], str)
                ):
                    out.append((str(item[0]), str(item[1])))
        self._pairs = out

    def get(self, name: str, default: object = None):
        needle = str(name).lower()
        for key, value in reversed(self._pairs):
            if key.lower() == needle:
                return value
        return default

    def get_all(self, name: str, failobj: object = None):
        needle = str(name).lower()
        values = [value for key, value in self._pairs if key.lower() == needle]
        return values if values else failobj

    def items(self) -> list[tuple[str, str]]:
        return list(self._pairs)

    def __iter__(self):
        for key, _ in self._pairs:
            yield key

    def __len__(self) -> int:
        return len(self._pairs)

    def __contains__(self, name: object) -> bool:
        if not isinstance(name, str):
            return False
        needle = name.lower()
        return any(key.lower() == needle for key, _ in self._pairs)

    def __getitem__(self, name: str) -> str:
        value = self.get(name, None)
        if value is None:
            raise KeyError(name)
        return str(value)


class HTTPServer(socketserver.TCPServer):
    pass


class ThreadingHTTPServer(socketserver.ThreadingTCPServer):
    pass


class BaseHTTPRequestHandler(socketserver.StreamRequestHandler):
    server_version = "BaseHTTP/0.6"
    sys_version = ""
    protocol_version = "HTTP/1.1"
    default_request_version = "HTTP/0.9"

    def handle(self) -> None:
        while bool(_MOLT_HTTP_SERVER_HANDLE_ONE_REQUEST(self)):
            pass

    def _molt_prepare_headers(self) -> None:
        pairs = getattr(self, "_molt_header_pairs", None)
        self.headers = _MoltHeaders(pairs)

    def send_response(self, code: int, message: str | None = None) -> None:
        _MOLT_HTTP_SERVER_SEND_RESPONSE(self, int(code), message)

    def send_response_only(self, code: int, message: str | None = None) -> None:
        _MOLT_HTTP_SERVER_SEND_RESPONSE_ONLY(self, int(code), message)

    def version_string(self) -> str:
        return str(
            _MOLT_HTTP_SERVER_VERSION_STRING(self.server_version, self.sys_version)
        )

    def date_time_string(self, timestamp: float | None = None) -> str:
        return str(_MOLT_HTTP_SERVER_DATE_TIME_STRING(timestamp))

    def send_header(self, keyword: str, value: str) -> None:
        _MOLT_HTTP_SERVER_SEND_HEADER(self, keyword, value)

    def end_headers(self) -> None:
        _MOLT_HTTP_SERVER_END_HEADERS(self)

    def send_error(self, code: int, message: str | None = None) -> None:
        _MOLT_HTTP_SERVER_SEND_ERROR(self, int(code), message)

    def log_message(self, format: str, *args: object) -> None:
        del format, args
        return None


class SimpleHTTPRequestHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802 - stdlib API name
        self.send_error(HTTPStatus.NOT_IMPLEMENTED.value, "GET not implemented")

    def do_HEAD(self) -> None:  # noqa: N802 - stdlib API name
        self.send_error(HTTPStatus.NOT_IMPLEMENTED.value, "HEAD not implemented")


class CGIHTTPRequestHandler(SimpleHTTPRequestHandler):
    def do_POST(self) -> None:  # noqa: N802 - stdlib API name
        self.send_error(HTTPStatus.NOT_IMPLEMENTED.value, "POST not implemented")


def executable(path: str) -> bool:
    return bool(os.path.isfile(path) and os.access(path, os.X_OK))


nobody = None


def nobody_uid() -> int | None:
    return None


def test(
    HandlerClass=BaseHTTPRequestHandler,
    ServerClass=ThreadingHTTPServer,
    protocol: str = "HTTP/1.0",
    port: int = 8000,
    bind: str | None = None,
) -> None:
    HandlerClass.protocol_version = protocol
    server_address = ("" if bind is None else str(bind), int(port))
    with ServerClass(server_address, HandlerClass) as httpd:
        httpd.serve_forever()


__all__ = [
    "BaseHTTPRequestHandler",
    "CGIHTTPRequestHandler",
    "DEFAULT_ERROR_CONTENT_TYPE",
    "DEFAULT_ERROR_MESSAGE",
    "HTTPServer",
    "HTTPStatus",
    "SimpleHTTPRequestHandler",
    "ThreadingHTTPServer",
    "copy",
    "datetime",
    "email",
    "executable",
    "html",
    "http",
    "io",
    "itertools",
    "mimetypes",
    "nobody",
    "nobody_uid",
    "os",
    "posixpath",
    "select",
    "shutil",
    "socket",
    "socketserver",
    "sys",
    "test",
    "time",
    "urllib",
]

globals().pop("_require_intrinsic", None)
