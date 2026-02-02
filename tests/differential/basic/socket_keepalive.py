# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket keepalive."""

import socket


sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
try:
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_KEEPALIVE, 1)
    val = sock.getsockopt(socket.SOL_SOCKET, socket.SO_KEEPALIVE)
    print(val in (0, 1))
except Exception as exc:
    print(type(exc).__name__)

sock.close()
