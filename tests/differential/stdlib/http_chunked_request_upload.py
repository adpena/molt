# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for http chunked request upload."""

import socket
import threading


ready = threading.Event()
port_holder: list[int] = []
body_seen: list[str] = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    conn.settimeout(1.0)
    chunks: list[bytes] = []
    while True:
        try:
            part = conn.recv(4096)
        except Exception:
            break
        if not part:
            break
        chunks.append(part)
        if b"\\r\\n0\\r\\n\\r\\n" in part:
            break
    data = b"".join(chunks).decode("latin-1")
    # Extract body after header separator
    parts = data.split("\r\n\r\n", 1)
    if len(parts) == 2:
        body_seen.append(parts[1])
    conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
req = (
    "POST /upload HTTP/1.1\r\n"
    "Host: 127.0.0.1\r\n"
    "Transfer-Encoding: chunked\r\n"
    "\r\n"
    "4\r\nWiki\r\n"
    "5\r\npedia\r\n"
    "0\r\n\r\n"
)

sock.sendall(req.encode("ascii"))
resp = sock.recv(1024)
sock.close()

t.join()

body = body_seen[0].strip().replace("\r\n", "|")
print("body", body)
print("resp", resp.split(b"\r\n", 1)[0].decode())
