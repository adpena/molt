# MOLT_ENV: MOLT_CAPABILITIES=net.outbound
"""Purpose: differential coverage for socket dns failure parity."""

import socket


try:
    socket.getaddrinfo("no-such-host.invalid", 80)
except Exception as exc:
    print(type(exc).__name__)
    if isinstance(exc, socket.gaierror):
        print(isinstance(exc.errno, int))
