# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for WSGI iterable close."""

import threading
import urllib.request
from wsgiref.simple_server import make_server

ready = threading.Event()
port_holder: list[int] = []
closed: list[bool] = []


class Body:
    def __iter__(self):
        yield b"ok"

    def close(self) -> None:
        closed.append(True)


def app(_environ, start_response):
    start_response("200 OK", [("Content-Length", "2")])
    return Body()


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
    _ = resp.read()

t.join(timeout=1.0)

print(bool(closed))
