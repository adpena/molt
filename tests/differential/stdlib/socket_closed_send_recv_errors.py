# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket closed send recv errors."""

import socket


sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.close()

try:
    sock.send(b"x")
except Exception as exc:
    print(type(exc).__name__)

try:
    sock.recv(1)
except Exception as exc:
    print(type(exc).__name__)
