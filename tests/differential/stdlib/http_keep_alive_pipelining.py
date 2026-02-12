# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for HTTP keep-alive pipelining."""

import http.server
import socket
import threading


ready = threading.Event()
port_holder: list[int] = []
seen: list[str] = []


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, _format: str, *args) -> None:
        return

    def do_GET(self) -> None:
        seen.append(self.path)
        body = f"path:{self.path}".encode("ascii")
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        if len(seen) < 2:
            self.send_header("Connection", "keep-alive")
        else:
            self.send_header("Connection", "close")
            self.close_connection = True
        self.end_headers()
        self.wfile.write(body)


def serve() -> None:
    server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
    port_holder.append(server.server_port)
    ready.set()
    server.handle_request()
    server.server_close()


def parse_responses(raw: bytes) -> list[tuple[str, str]]:
    results: list[tuple[str, str]] = []
    idx = 0
    while True:
        header_end = raw.find(b"\r\n\r\n", idx)
        if header_end == -1:
            break
        header = raw[idx:header_end]
        lines = header.split(b"\r\n")
        status = lines[0].decode("ascii", errors="replace")
        length = 0
        for line in lines[1:]:
            if line.lower().startswith(b"content-length:"):
                length = int(line.split(b":", 1)[1].strip() or b"0")
                break
        body_start = header_end + 4
        body_end = body_start + length
        if body_end > len(raw):
            break
        body = raw[body_start:body_end].decode("ascii", errors="replace")
        results.append((status, body))
        idx = body_end
    return results


t = threading.Thread(target=serve)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
requests = (
    b"GET /one HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n"
    b"GET /two HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
)
sock.sendall(requests)
sock.shutdown(socket.SHUT_WR)
raw = b""
while True:
    chunk = sock.recv(4096)
    if not chunk:
        break
    raw += chunk
sock.close()
t.join(timeout=1.0)

print(parse_responses(raw))
