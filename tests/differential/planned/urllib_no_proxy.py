# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for urllib no_proxy behavior."""

import os
import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer

ready = threading.Event()
port_holder: list[int] = []


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"ok")

    def log_message(self, _fmt, *_args):
        return


def serve() -> None:
    httpd = HTTPServer(("127.0.0.1", 0), Handler)
    port_holder.append(httpd.server_port)
    ready.set()
    httpd.handle_request()
    httpd.server_close()


t = threading.Thread(target=serve)
t.start()
ready.wait(timeout=1.0)

prev_http = os.environ.get("http_proxy")
prev_no_proxy = os.environ.get("no_proxy")
os.environ["http_proxy"] = "http://127.0.0.1:9"
os.environ["no_proxy"] = "127.0.0.1"

try:
    with urllib.request.urlopen(
        f"http://127.0.0.1:{port_holder[0]}/", timeout=1.0
    ) as resp:
        body = resp.read().decode("ascii", errors="replace")
        print(body)
finally:
    if prev_http is None:
        os.environ.pop("http_proxy", None)
    else:
        os.environ["http_proxy"] = prev_http
    if prev_no_proxy is None:
        os.environ.pop("no_proxy", None)
    else:
        os.environ["no_proxy"] = prev_no_proxy

t.join(timeout=1.0)
