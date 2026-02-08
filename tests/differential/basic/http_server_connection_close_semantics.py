# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: connection-close flags should not poison subsequent requests."""

import http.server
import threading
import urllib.request


class Handler(http.server.BaseHTTPRequestHandler):
    calls = 0

    def do_GET(self) -> None:
        type(self).calls += 1
        if self.path == "/close":
            self.close_connection = True
        body = f"#{type(self).calls}".encode("ascii")
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
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/close", timeout=1.0) as resp:
        print(resp.status, resp.read().decode())
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/next", timeout=1.0) as resp:
        print(resp.status, resp.read().decode())
finally:
    server.shutdown()
    thread.join()
