# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket tcp nodelay."""

import socket


sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
try:
    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    val = sock.getsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY)
    print(val in (0, 1))
except Exception as exc:
    print(type(exc).__name__)

sock.close()
