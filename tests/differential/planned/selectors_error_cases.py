# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors error cases."""

import selectors
import socket


sel = selectors.DefaultSelector()

try:
    a, b = socket.socketpair()
except AttributeError:
    print("no_socketpair")
else:
    sel.register(a, selectors.EVENT_READ)
    try:
        sel.register(a, selectors.EVENT_READ)
    except Exception as exc:
        print(type(exc).__name__)

    sel.unregister(a)
    try:
        sel.unregister(a)
    except Exception as exc:
        print(type(exc).__name__)

    a.close()
    b.close()
