"""Purpose: urllib.response HTTP stream semantics match CPython with intrinsic lowering."""

import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

import urllib.request


class _Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        body = b"abc\ndef\n"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        del fmt, args


server = HTTPServer(("127.0.0.1", 0), _Handler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
url = f"http://127.0.0.1:{server.server_port}/"

try:
    with urllib.request.urlopen(url, timeout=1.0) as resp:
        print("caps", resp.readable(), resp.writable(), resp.seekable())
        for name, op in (("tell", lambda: resp.tell()), ("seek", lambda: resp.seek(0))):
            try:
                value = op()
                print(name, "ok", value)
            except Exception as exc:  # noqa: BLE001
                print(name, type(exc).__name__, str(exc))
        print("read1", resp.read1(2).decode("ascii"))
        out = bytearray(2)
        print("readinto1", resp.readinto1(out), bytes(out).decode("ascii"))

    closed = urllib.request.urlopen(url, timeout=1.0)
    closed.close()
    for name, op in (
        ("read", lambda: closed.read().decode("ascii")),
        ("readline", lambda: closed.readline().decode("ascii")),
        ("readlines", lambda: [line.decode("ascii") for line in closed.readlines()]),
        ("readinto", lambda: closed.readinto(bytearray(2))),
        ("read1", lambda: closed.read1().decode("ascii")),
        ("readinto1", lambda: closed.readinto1(bytearray(2))),
        ("readable", lambda: closed.readable()),
        ("writable", lambda: closed.writable()),
        ("seekable", lambda: closed.seekable()),
    ):
        try:
            value = op()
            print("closed", name, "ok", value)
        except Exception as exc:  # noqa: BLE001
            print("closed", name, type(exc).__name__, str(exc))

    for name, op in (("tell", lambda: closed.tell()), ("seek", lambda: closed.seek(0))):
        try:
            value = op()
            print("closed", name, "ok", value)
        except Exception as exc:  # noqa: BLE001
            print("closed", name, type(exc).__name__, str(exc))
finally:
    server.shutdown()
    server.server_close()
    thread.join(timeout=1.0)
