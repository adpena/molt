# MOLT_ENV: MOLT_CAPABILITIES=net.outbound
"""Purpose: differential coverage for socket reverse dns."""

import socket


try:
    name, aliases, addrs = socket.gethostbyaddr("127.0.0.1")
    print(isinstance(name, str), isinstance(aliases, list), isinstance(addrs, list))
except Exception as exc:
    print(type(exc).__name__)
