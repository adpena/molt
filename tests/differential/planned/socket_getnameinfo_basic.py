# MOLT_ENV: MOLT_CAPABILITIES=net.outbound
"""Purpose: differential coverage for socket getnameinfo basic."""

import socket


try:
    res = socket.getnameinfo(("127.0.0.1", 80), 0)
    print(isinstance(res, tuple), len(res))
except Exception as exc:
    print(type(exc).__name__)
