# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors basic."""

import selectors
import socket


sel = selectors.DefaultSelector()

try:
    a, b = socket.socketpair()
except AttributeError:
    print("no_socketpair")
else:
    sel.register(a, selectors.EVENT_READ)
    b.send(b"x")
    events = sel.select(timeout=1.0)
    print(len(events) == 1)
    key, mask = events[0]
    print(key.fileobj is a, bool(mask & selectors.EVENT_READ))
    a.close()
    b.close()
