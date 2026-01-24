# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for HTTP HEAD without body."""

import http.client
import http.server
import threading


ready = threading.Event()
port_holder: list[int] = []


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, _format: str, *args) -> None:
        return

    def do_HEAD(self) -> None:
        body_len = 5
        self.send_response(200)
        self.send_header("Content-Length", str(body_len))
        self.end_headers()


def serve() -> None:
    server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
    port_holder.append(server.server_port)
    ready.set()
    server.handle_request()
    server.server_close()


t = threading.Thread(target=serve)
t.start()
ready.wait(timeout=1.0)

conn = http.client.HTTPConnection("127.0.0.1", port_holder[0], timeout=1.0)
conn.request("HEAD", "/")
resp = conn.getresponse()
body = resp.read()
conn.close()
t.join(timeout=1.0)

print(resp.status, resp.getheader("Content-Length"), len(body))
