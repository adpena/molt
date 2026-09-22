# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for gzip + chunked response."""

import gzip
import socket
import threading

ready = threading.Event()
port_holder: list[int] = []


def chunk(data: bytes) -> bytes:
    return f"{len(data):x}".encode("ascii") + b"\r\n" + data + b"\r\n"


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()
    conn, _addr = srv.accept()
    conn.recv(1024)
    body = gzip.compress(b"hello")
    resp = (
        b"HTTP/1.1 200 OK\r\n"
        b"Content-Encoding: gzip\r\n"
        b"Transfer-Encoding: chunked\r\n\r\n" + chunk(body) + b"0\r\n\r\n"
    )
    conn.sendall(resp)
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
request = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"
sock.sendall(request)
response = sock.recv(4096)
sock.close()

t.join(timeout=1.0)

_header, body = response.split(b"\r\n\r\n", 1)
# One data chunk followed by the terminating zero-length chunk: decode it by
# its declared size, since the compressed payload may itself contain CRLF.
size_line, rest = body.split(b"\r\n", 1)
size = int(size_line, 16)
raw = rest[:size]
assert rest[size:] == b"\r\n0\r\n\r\n"
print(gzip.decompress(raw))
