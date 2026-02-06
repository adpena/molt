"""Minimal intrinsic-friendly http.server subset for Molt."""

from __future__ import annotations


from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

import socketserver


_REASON_PHRASES = {
    101: "Switching Protocols",
    200: "OK",
    400: "Bad Request",
    404: "Not Found",
    500: "Internal Server Error",
    501: "Not Implemented",
}


class HTTPServer(socketserver.TCPServer):
    pass


class ThreadingHTTPServer(HTTPServer):
    pass


class BaseHTTPRequestHandler(socketserver.StreamRequestHandler):
    server_version = "BaseHTTP/0.6"
    sys_version = ""
    protocol_version = "HTTP/1.1"

    def handle(self) -> None:
        request_line = self.rfile.readline(65537)
        if not request_line:
            return
        text = request_line.decode("iso-8859-1", "surrogateescape").rstrip("\r\n")
        parts = text.split()
        if len(parts) < 3:
            return
        self.command = parts[0]
        self.path = parts[1]
        self.request_version = parts[2]
        self.headers: dict[str, str] = {}
        while True:
            line = self.rfile.readline(65537)
            if not line or line in (b"\r\n", b"\n"):
                break
            line_text = line.decode("iso-8859-1", "surrogateescape").rstrip("\r\n")
            if ":" not in line_text:
                continue
            key, value = line_text.split(":", 1)
            self.headers[key.strip()] = value.lstrip()
        method_name = f"do_{self.command}"
        handler = getattr(self, method_name, None)
        if handler is None:
            self.send_response(501)
            self.end_headers()
            return
        handler()

    def send_response(self, code: int, message: str | None = None) -> None:
        reason = message if message is not None else _REASON_PHRASES.get(code, "")
        line = f"HTTP/1.1 {int(code)} {reason}\r\n".encode("ascii", "surrogateescape")
        self.wfile.write(line)

    def send_header(self, keyword: str, value: str) -> None:
        line = f"{keyword}: {value}\r\n".encode("ascii", "surrogateescape")
        self.wfile.write(line)

    def end_headers(self) -> None:
        self.wfile.write(b"\r\n")
        if hasattr(self.wfile, "flush"):
            self.wfile.flush()

    def log_message(self, format: str, *args: object) -> None:
        del format, args
        return None


__all__ = [
    "BaseHTTPRequestHandler",
    "HTTPServer",
    "ThreadingHTTPServer",
]
