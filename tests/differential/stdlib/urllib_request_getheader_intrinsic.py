# MOLT_META: wasm=no
# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: urllib.request response getheader is intrinsic-backed and preserves joins."""

import http.server
import threading
import urllib.request


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        body = b"ok"
        self.send_response(200)
        self.send_header("X-Test", "one")
        self.send_header("X-Test", "two")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *args) -> None:
        return None


server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
thread = threading.Thread(target=server.serve_forever)
thread.daemon = True
thread.start()

host, port = server.server_address
with urllib.request.urlopen(f"http://{host}:{port}/") as resp:
    print("code", resp.getcode())
    print("x_joined", resp.getheader("X-Test"))
    print("x_all", [value for key, value in resp.getheaders() if key == "X-Test"])
    print("x_missing", resp.getheader("X-Missing", "fallback"))
    print("body", resp.read().decode("ascii"))

server.shutdown()
thread.join(timeout=1.0)
server.server_close()
