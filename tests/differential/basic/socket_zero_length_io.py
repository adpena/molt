"""Purpose: differential coverage for zero-length send/recv."""

import socket

try:
    a, b = socket.socketpair()
except AttributeError:
    print("no_socketpair")
else:
    b.sendall(b"hi")
    print(a.recv(0), b.recv(0))
    a.close()
    b.close()
