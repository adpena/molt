# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket.socketpair."""

import socket


if not hasattr(socket, "socketpair"):
    print("missing socketpair")
else:
    left, right = socket.socketpair()
    left.sendall(b"ping")
    print(right.recv(4).decode())
    right.sendall(b"pong")
    print(left.recv(4).decode())
    left.close()
    right.close()
