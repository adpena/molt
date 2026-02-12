# MOLT_META: wasm=no
# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: intrinsic-backed http.client should preserve header semantics."""

import http.client
import http.server
import threading


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        body = b"ok"
        self.send_response(200)
        self.send_header("X-Test", "a")
        self.send_header("X-Test", "b")
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
conn = http.client.HTTPConnection(host, port, timeout=2.0)
conn.request("GET", "/")
resp = conn.getresponse()
print("status", resp.status)
print("x_test", resp.getheader("X-Test"))
print("headers_len", len(resp.getheaders()))
print("body", resp.read().decode("ascii"))
resp.close()
conn.close()

server.shutdown()
thread.join(timeout=1.0)
server.server_close()
