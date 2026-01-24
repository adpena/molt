# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for redirect method changes."""

import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer

ready = threading.Event()
port_holder: list[int] = []
methods: list[str] = []


class Handler(BaseHTTPRequestHandler):
    def do_POST(self) -> None:
        if self.path == "/start":
            self.send_response(303)
            self.send_header("Location", "/target")
            self.end_headers()
        else:
            methods.append("POST")
            self.send_response(200)
            self.end_headers()

    def do_GET(self) -> None:
        if self.path == "/target":
            methods.append("GET")
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok")
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, _fmt, *_args):
        return


def serve() -> None:
    httpd = HTTPServer(("127.0.0.1", 0), Handler)
    port_holder.append(httpd.server_port)
    ready.set()
    httpd.handle_request()
    httpd.handle_request()
    httpd.server_close()


t = threading.Thread(target=serve)
t.start()
ready.wait(timeout=1.0)

req = urllib.request.Request(
    f"http://127.0.0.1:{port_holder[0]}/start", data=b"ping"
)
with urllib.request.urlopen(req, timeout=1.0) as resp:
    _ = resp.read()

t.join(timeout=1.0)

print(methods)
