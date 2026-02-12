# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors modify get key errors."""

import selectors
import socket


sel = selectors.DefaultSelector()

try:
    a, b = socket.socketpair()
except AttributeError:
    print("no_socketpair")
else:
    try:
        sel.modify(a, selectors.EVENT_READ)
    except Exception as exc:
        print(type(exc).__name__)

    try:
        sel.get_key(a)
    except Exception as exc:
        print(type(exc).__name__)

    a.close()
    b.close()
