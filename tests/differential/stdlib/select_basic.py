# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for select basic."""

import select
import socket


try:
    a, b = socket.socketpair()
    b.send(b"hi")
    r, w, x = select.select([a], [], [], 1.0)
    print(len(r), len(w), len(x))
    print(a.recv(2).decode())
    a.close()
    b.close()
except AttributeError:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    srv.listen(1)
    port = srv.getsockname()[1]

    client = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    client.connect(("127.0.0.1", port))
    conn, _addr = srv.accept()
    client.sendall(b"hi")

    r, w, x = select.select([conn], [], [], 1.0)
    print(len(r), len(w), len(x))
    print(conn.recv(2).decode())

    client.close()
    conn.close()
    srv.close()
