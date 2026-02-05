# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors register unregister."""

import selectors
import socket


sel = selectors.DefaultSelector()

try:
    a, b = socket.socketpair()
except AttributeError:
    print("no_socketpair")
else:
    key = sel.register(a, selectors.EVENT_READ)
    print(key.fileobj is a)
    sel.unregister(a)
    events = sel.select(timeout=0.01)
    print(events == [])
    a.close()
    b.close()
