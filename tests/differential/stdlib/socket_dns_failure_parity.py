# MOLT_ENV: MOLT_CAPABILITIES=net
"""Purpose: resolver errors retain one canonical class through both socket modules."""

import socket
import _socket


# AI_NUMERICHOST makes this failure deterministic and avoids external DNS.
for module in (socket, _socket):
    try:
        module.getaddrinfo("not-a-numeric-address.invalid", 80, 0, 0, 0, socket.AI_NUMERICHOST)
    except socket.gaierror as exc:
        print("resolver", type(exc) is socket.gaierror, type(exc) is _socket.gaierror)
        print("resolver_code", exc.errno == socket.EAI_NONAME, isinstance(exc.strerror, str))
    else:
        print("unexpected-success")
