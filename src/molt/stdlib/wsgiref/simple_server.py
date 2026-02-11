"""Minimal `wsgiref.simple_server` subset for Molt."""

from __future__ import annotations

import io
import socketserver
import sys
from typing import Any, Callable
from urllib.parse import urlsplit, unquote

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_WSGIREF_RUNTIME_READY = _require_intrinsic(
    "molt_wsgiref_runtime_ready", globals()
)

StartResponse = Callable[
    [str, list[tuple[str, str]], Any | None], Callable[[bytes], None]
]
WSGIApplication = Callable[[dict[str, Any], StartResponse], Any]


class WSGIRequestHandler(socketserver.StreamRequestHandler):
    server: "WSGIServer"

    def handle(self) -> None:
        request_line = self.rfile.readline()
        if not request_line:
            return None
        try:
            decoded_line = request_line.decode("iso-8859-1").strip()
        except Exception:
            return None
        parts = decoded_line.split()
        if len(parts) != 3:
            self._write_raw_response("400 Bad Request", [("Content-Length", "0")], b"")
            return None
        method, target, protocol = parts
        headers = self._read_headers()
        body = b""
        content_length_raw = None
        for header_name, header_value in headers.items():
            if header_name.lower() == "content-length":
                content_length_raw = header_value
                break
        if content_length_raw:
            try:
                content_length = max(0, int(content_length_raw))
            except Exception:
                content_length = 0
            if content_length:
                body = bytes(self.rfile.read(content_length))
        environ = self._build_environ(method, target, protocol, headers, body)

        status_line = "500 Internal Server Error"
        response_headers: list[tuple[str, str]] = []
        body_parts: list[bytes] = []

        def write(chunk: bytes) -> None:
            body_parts.append(bytes(chunk))

        def start_response(
            status: str,
            provided_headers: list[tuple[str, str]],
            _exc_info: Any | None = None,
        ) -> Callable[[bytes], None]:
            nonlocal status_line, response_headers
            status_line = str(status)
            response_headers = [
                (str(header_name), str(header_value))
                for header_name, header_value in provided_headers
            ]
            return write

        result = self.server.application(environ, start_response)
        try:
            for chunk in result:
                if chunk:
                    write(bytes(chunk))
        finally:
            close = getattr(result, "close", None)
            if callable(close):
                close()

        response_body = b"".join(body_parts)
        lowered_names = {header_name.lower() for header_name, _ in response_headers}
        if "content-length" not in lowered_names:
            response_headers.append(("Content-Length", str(len(response_body))))
        self._write_raw_response(status_line, response_headers, response_body)
        return None

    def _read_headers(self) -> dict[str, str]:
        headers: dict[str, str] = {}
        while True:
            raw_line = self.rfile.readline()
            if not raw_line or raw_line in (b"\n", b"\r\n"):
                break
            try:
                line = raw_line.decode("iso-8859-1").rstrip("\r\n")
            except Exception:
                continue
            if ":" not in line:
                continue
            name, value = line.split(":", 1)
            headers[name.strip()] = value.strip()
        return headers

    def _build_environ(
        self,
        method: str,
        target: str,
        protocol: str,
        headers: dict[str, str],
        body: bytes,
    ) -> dict[str, Any]:
        parsed = urlsplit(target)
        path_info = parsed.path or "/"
        query_string = parsed.query

        environ: dict[str, Any] = {}
        environ["SERVER_NAME"] = "127.0.0.1"
        environ["SERVER_PROTOCOL"] = "HTTP/1.0"
        environ["HTTP_HOST"] = environ["SERVER_NAME"]
        environ["REQUEST_METHOD"] = "GET"
        environ["SCRIPT_NAME"] = ""
        environ["PATH_INFO"] = "/"
        environ["QUERY_STRING"] = ""
        environ["CONTENT_TYPE"] = "text/plain"
        environ["CONTENT_LENGTH"] = str(len(body))
        environ["SERVER_PORT"] = "80"
        environ["wsgi.version"] = (1, 0)
        environ["wsgi.url_scheme"] = "http"
        environ["wsgi.input"] = io.BytesIO(body)
        environ["wsgi.errors"] = sys.stderr
        environ["wsgi.multithread"] = False
        environ["wsgi.multiprocess"] = False
        environ["wsgi.run_once"] = False
        environ["REQUEST_METHOD"] = method
        environ["SCRIPT_NAME"] = ""
        environ["PATH_INFO"] = unquote(path_info)
        environ["QUERY_STRING"] = query_string
        environ["SERVER_PROTOCOL"] = protocol
        environ["SERVER_NAME"] = str(self.server.server_name)
        environ["SERVER_PORT"] = str(self.server.server_port)
        environ["wsgi.url_scheme"] = "http"
        # Preserve request payload for POST/PUT handlers (for example XML-RPC).
        environ["wsgi.input"] = io.BytesIO(body)
        environ["wsgi.errors"] = sys.stderr
        environ["wsgi.multithread"] = True
        environ["wsgi.multiprocess"] = False
        environ["wsgi.run_once"] = False

        for name, value in headers.items():
            upper_name = name.upper().replace("-", "_")
            if upper_name == "CONTENT_TYPE":
                environ["CONTENT_TYPE"] = value
            elif upper_name == "CONTENT_LENGTH":
                environ["CONTENT_LENGTH"] = value
            else:
                environ[f"HTTP_{upper_name}"] = value

        if "Host" in headers:
            environ["HTTP_HOST"] = headers["Host"]
        return environ

    def _write_raw_response(
        self,
        status_line: str,
        headers: list[tuple[str, str]],
        body: bytes,
    ) -> None:
        wire = [f"HTTP/1.1 {status_line}\r\n".encode("iso-8859-1")]
        for header_name, header_value in headers:
            wire.append(f"{header_name}: {header_value}\r\n".encode("iso-8859-1"))
        wire.append(b"\r\n")
        wire.append(body)
        self.wfile.write(b"".join(wire))
        self.wfile.flush()


class WSGIServer(socketserver.TCPServer):
    def __init__(
        self,
        server_address: tuple[str, int],
        RequestHandlerClass: type[WSGIRequestHandler],
    ) -> None:
        super().__init__(server_address, RequestHandlerClass)
        self.application: WSGIApplication = lambda _environ, _start_response: []

    def set_app(self, application: WSGIApplication) -> None:
        self.application = application


def make_server(
    host: str,
    port: int,
    app: WSGIApplication,
    server_class: type[WSGIServer] = WSGIServer,
    handler_class: type[WSGIRequestHandler] = WSGIRequestHandler,
) -> WSGIServer:
    httpd = server_class((host, int(port)), handler_class)
    httpd.set_app(app)
    return httpd


__all__ = ["WSGIServer", "WSGIRequestHandler", "make_server"]
