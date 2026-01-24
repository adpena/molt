# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for Content-Length vs close."""

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
    conn.recv(1024)
    body = b"hello"
    resp = (
        b"HTTP/1.1 200 OK
"
        b"Content-Length: 5
"
        b"Connection: close

"
        + body
    )
    conn.sendall(resp)
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
request = b"GET / HTTP/1.1
Host: localhost

"
sock.sendall(request)
response = sock.recv(1024)
sock.close()

t.join(timeout=1.0)

print(response.split(b"

", 1)[1])
