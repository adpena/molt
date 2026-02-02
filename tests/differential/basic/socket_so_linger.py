# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket so linger."""

import socket


sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
try:
    sock.setsockopt(
        socket.SOL_SOCKET, socket.SO_LINGER, b"\x00\x00\x00\x00\x00\x00\x00\x00"
    )
    print(True)
except Exception as exc:
    print(type(exc).__name__)

sock.close()
