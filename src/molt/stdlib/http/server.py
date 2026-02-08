"""Minimal intrinsic-first http.server subset for Molt."""

from __future__ import annotations

import socketserver

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_HTTP_SERVER_READ_REQUEST = _require_intrinsic(
    "molt_http_server_read_request", globals()
)
_MOLT_HTTP_SERVER_COMPUTE_CLOSE_CONNECTION = _require_intrinsic(
    "molt_http_server_compute_close_connection", globals()
)
_MOLT_HTTP_SERVER_HANDLE_ONE_REQUEST = _require_intrinsic(
    "molt_http_server_handle_one_request", globals()
)
_MOLT_HTTP_SERVER_SEND_RESPONSE = _require_intrinsic(
    "molt_http_server_send_response", globals()
)
_MOLT_HTTP_SERVER_SEND_RESPONSE_ONLY = _require_intrinsic(
    "molt_http_server_send_response_only", globals()
)
_MOLT_HTTP_SERVER_SEND_HEADER = _require_intrinsic(
    "molt_http_server_send_header", globals()
)
_MOLT_HTTP_SERVER_END_HEADERS = _require_intrinsic(
    "molt_http_server_end_headers", globals()
)
_MOLT_HTTP_SERVER_SEND_ERROR = _require_intrinsic(
    "molt_http_server_send_error", globals()
)
_MOLT_HTTP_SERVER_VERSION_STRING = _require_intrinsic(
    "molt_http_server_version_string", globals()
)
_MOLT_HTTP_SERVER_DATE_TIME_STRING = _require_intrinsic(
    "molt_http_server_date_time_string", globals()
)


class HTTPServer(socketserver.TCPServer):
    pass


class ThreadingHTTPServer(HTTPServer):
    pass


class BaseHTTPRequestHandler(socketserver.StreamRequestHandler):
    server_version = "BaseHTTP/0.6"
    sys_version = ""
    protocol_version = "HTTP/1.1"
    default_request_version = "HTTP/0.9"

    def handle(self) -> None:
        while bool(_MOLT_HTTP_SERVER_HANDLE_ONE_REQUEST(self)):
            pass

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


__all__ = [
    "BaseHTTPRequestHandler",
    "HTTPServer",
    "ThreadingHTTPServer",
]
