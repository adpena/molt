# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for urllib cookie jar persistence."""

import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer
from http.cookiejar import CookieJar

ready = threading.Event()
port_holder: list[int] = []
received: list[str] = []


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path == "/set":
            self.send_response(200)
            self.send_header("Set-Cookie", "session=abc")
            self.end_headers()
            self.wfile.write(b"ok")
        elif self.path == "/check":
            received.append(self.headers.get("Cookie", ""))
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

jar = CookieJar()
opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))

with opener.open(f"http://127.0.0.1:{port_holder[0]}/set", timeout=1.0) as resp:
    _ = resp.read()

with opener.open(f"http://127.0.0.1:{port_holder[0]}/check", timeout=1.0) as resp:
    _ = resp.read()

t.join(timeout=1.0)

print(received)
