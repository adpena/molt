# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for WSGI basic server."""

import threading
import urllib.request
from wsgiref.simple_server import make_server


ready = threading.Event()
port_holder: list[int] = []


def app(_environ, start_response):
    body = b"hello"
    start_response("200 OK", [("Content-Type", "text/plain"), ("Content-Length", "5")])
    return [body]


def serve() -> None:
    httpd = make_server("127.0.0.1", 0, app)
    port_holder.append(httpd.server_port)
    ready.set()
    httpd.handle_request()
    httpd.server_close()


t = threading.Thread(target=serve)
t.start()
ready.wait(timeout=1.0)

with urllib.request.urlopen(
    f"http://127.0.0.1:{port_holder[0]}/", timeout=1.0
) as resp:
    body = resp.read().decode("ascii", errors="replace")
    status = resp.getcode()

t.join(timeout=1.0)

print(status, body)
