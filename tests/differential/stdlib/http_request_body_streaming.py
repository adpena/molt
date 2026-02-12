# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for HTTP request body streaming."""

import http.client
import http.server
import threading


ready = threading.Event()
port_holder: list[int] = []
received: list[str] = []


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, _format: str, *args) -> None:
        return

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        remaining = length
        chunks: list[int] = []
        data = b""
        while remaining:
            size = 3 if remaining >= 3 else remaining
            piece = self.rfile.read(size)
            chunks.append(len(piece))
            data += piece
            remaining -= len(piece)
        received.append(f"{chunks}:{data.decode('ascii', errors='replace')}")
        body = received[0].encode("ascii")
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def serve() -> None:
    server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
    port_holder.append(server.server_port)
    ready.set()
    server.handle_request()
    server.server_close()


t = threading.Thread(target=serve)
t.start()
ready.wait(timeout=1.0)

payload = b"abcdefghij"
conn = http.client.HTTPConnection("127.0.0.1", port_holder[0], timeout=1.0)
conn.putrequest("POST", "/")
conn.putheader("Content-Length", str(len(payload)))
conn.endheaders()
conn.send(payload[:4])
conn.send(payload[4:])
resp = conn.getresponse()
body = resp.read().decode("ascii", errors="replace")
conn.close()
t.join(timeout=1.0)

print(body)
