# MOLT_ENV: MOLT_CAPABILITIES=net.outbound
"""Purpose: differential coverage for socket service lookup."""

import socket


try:
    print(socket.getservbyname("http"))
except Exception as exc:
    print(type(exc).__name__)

try:
    print(socket.getservbyport(80))
except Exception as exc:
    print(type(exc).__name__)
