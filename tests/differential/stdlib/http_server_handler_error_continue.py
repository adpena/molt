# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: handler exceptions should not kill serve_forever."""

import http.server
import threading
import urllib.request


class Handler(http.server.BaseHTTPRequestHandler):
    calls = 0

    def do_GET(self) -> None:
        type(self).calls += 1
        if self.path == "/err":
            raise RuntimeError("handler_fail")
        body = b"OK"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *args) -> None:
        return None


server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
port = server.server_address[1]

thread = threading.Thread(target=server.serve_forever)
thread.daemon = True
thread.start()

try:
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/err", timeout=1.0):
            print("first_ok")
    except Exception:
        print("first_error")
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/ok", timeout=1.0) as resp:
        print(resp.status, resp.read().decode())
finally:
    server.shutdown()
    thread.join()
