# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for http server basic."""

import http.server
import threading
import urllib.request


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
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

with urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=1.0) as resp:
    body = resp.read().decode()
    print(resp.status, body)

server.shutdown()
thread.join()
