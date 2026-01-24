# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for HTTP-like socket exchange."""

import socket
import threading


ready = threading.Event()
port_holder: list[int] = []
seen_request: list[str] = []


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
        data = conn.recv(1024)
        if not data:
            break
        chunks.append(data)
        if b"\r\n\r\n" in b"".join(chunks):
            break
    raw = b"".join(chunks)
    line = raw.split(b"\r\n", 1)[0].decode("ascii", errors="replace")
    seen_request.append(line)
    response = (
        b"HTTP/1.1 200 OK\r\n"
        b"Content-Length: 5\r\n"
        b"Connection: close\r\n"
        b"\r\n"
        b"hello"
    )
    conn.sendall(response)
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
request = b"GET / HTTP/1.1\r\nHost: example\r\n\r\n"
sock.sendall(request[:10])
sock.sendall(request[10:])
chunks: list[bytes] = []
while True:
    data = sock.recv(1024)
    if not data:
        break
    chunks.append(data)
sock.close()
t.join()

raw = b"".join(chunks)
status_line = raw.split(b"\r\n", 1)[0].decode("ascii", errors="replace")
body = raw.split(b"\r\n\r\n", 1)[1].decode("ascii", errors="replace")
print(seen_request[0])
print(status_line)
print(body)
