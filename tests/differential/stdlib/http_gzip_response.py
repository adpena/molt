# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for http gzip response."""

import gzip
import socket
import threading


ready = threading.Event()
port_holder: list[int] = []


def server() -> None:
    payload = gzip.compress(b"hello")
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    conn.recv(1024)
    conn.sendall(
        b"HTTP/1.1 200 OK\r\n"
        b"Content-Encoding: gzip\r\n"
        + f"Content-Length: {len(payload)}\r\n".encode("ascii")
        + b"Connection: close\r\n"
        + b"\r\n"
        + payload
    )
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
req = "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept-Encoding: gzip\r\n\r\n"

sock.sendall(req.encode("ascii"))
raw = sock.recv(4096)
sock.close()

t.join()

header, body = raw.split(b"\r\n\r\n", 1)
print(header.split(b"\r\n", 1)[0].decode())
print(gzip.decompress(body).decode())
