# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for HTTP 100-continue."""

import socket
import threading

ready = threading.Event()
port_holder: list[int] = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()
    conn, _addr = srv.accept()
    data = conn.recv(1024)
    if b"Expect: 100-continue" in data:
        conn.sendall(b"HTTP/1.1 100 Continue

")
    body = conn.recv(1024)
    resp = b"HTTP/1.1 200 OK
Content-Length: 2

OK"
    conn.sendall(resp)
    conn.close()
    srv.close()
    print(body)


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
request = (
    b"POST / HTTP/1.1
"
    b"Host: localhost
"
    b"Content-Length: 4
"
    b"Expect: 100-continue

"
)
sock.sendall(request)
interim = sock.recv(64)
if b"100 Continue" in interim:
    sock.sendall(b"ping")
final = sock.recv(128)
sock.close()

t.join(timeout=1.0)

print(b"100" in interim, b"200" in final)
