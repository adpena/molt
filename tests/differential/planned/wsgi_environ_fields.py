# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for WSGI environ fields."""

import threading
import urllib.request
from wsgiref.simple_server import make_server


ready = threading.Event()
port_holder: list[int] = []


def app(environ, start_response):
    fields = [
        environ.get("REQUEST_METHOD", ""),
        environ.get("PATH_INFO", ""),
        environ.get("QUERY_STRING", ""),
        environ.get("SERVER_PROTOCOL", ""),
        environ.get("wsgi.url_scheme", ""),
        environ.get("HTTP_HOST", ""),
    ]
    body = "|".join(fields).encode("ascii", errors="replace")
    start_response("200 OK", [("Content-Length", str(len(body)))])
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

url = f"http://127.0.0.1:{port_holder[0]}/path/info?x=1&y=2"
req = urllib.request.Request(url, headers={"Host": "example.com"})
with urllib.request.urlopen(req, timeout=1.0) as resp:
    body = resp.read().decode("ascii", errors="replace")

t.join(timeout=1.0)

print(body)
