# MOLT_ENV: MOLT_CAPABILITIES=net.outbound
"""Purpose: differential coverage for socket dns basic."""

import socket


try:
    addr = socket.gethostbyname("localhost")
    print(addr)
except Exception as exc:
    print(type(exc).__name__)

try:
    info = socket.getaddrinfo("localhost", 80)
    print(len(info) > 0)
except Exception as exc:
    print(type(exc).__name__)
