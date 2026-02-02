# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for httpclient chunked iter upload."""

import http.client
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
        if b"\r\n0\r\n\r\n" in part:
            break
    data = b"".join(chunks).decode("latin-1")
    parts = data.split("\r\n\r\n", 1)
    if len(parts) == 2:
        body_seen.append(parts[1])
    conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
    conn.close()
    srv.close()


def gen() -> list[bytes]:
    return [b"Wiki", b"pedia"]


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

conn = http.client.HTTPConnection("127.0.0.1", port_holder[0], timeout=1.0)
conn.request("POST", "/", body=gen(), headers={"Transfer-Encoding": "chunked"})
resp = conn.getresponse()
resp.read()
conn.close()

t.join()

body = body_seen[0].strip().replace("\r\n", "|")
print("body", body)
