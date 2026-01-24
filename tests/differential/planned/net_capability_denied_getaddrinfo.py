# MOLT_ENV: MOLT_CAPABILITIES=
"""Purpose: differential coverage for net capability denied getaddrinfo."""

import socket


try:
    info = socket.getaddrinfo("localhost", 0)
    print("count", len(info))
except Exception as exc:
    print(type(exc).__name__)
