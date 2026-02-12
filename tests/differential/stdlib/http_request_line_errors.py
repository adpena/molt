# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for http request line errors."""

import http.server
import socket
import threading


ready = threading.Event()
port_holder: list[int] = []


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, _format: str, *args) -> None:
        return


def serve() -> None:
    server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
    port_holder.append(server.server_port)
    ready.set()
    for _ in range(2):
        server.handle_request()
    server.server_close()


def send_request(raw: bytes) -> str:
    sock = socket.create_connection(("127.0.0.1", port_holder[0]))
    sock.sendall(raw)
    sock.shutdown(socket.SHUT_WR)
    data = b""
    while True:
        chunk = sock.recv(1024)
        if not chunk:
            break
        data += chunk
    sock.close()
    return data.split(b"\r\n", 1)[0].decode("ascii", errors="replace")


t = threading.Thread(target=serve)
t.start()
ready.wait(timeout=1.0)

status1 = send_request(b"GET / HTTP/1.1 extra\r\nHost: example\r\n\r\n")
status2 = send_request(b"GET / HTTX/1.1\r\nHost: example\r\n\r\n")

t.join(timeout=1.0)

print(status1)
print(status2)
