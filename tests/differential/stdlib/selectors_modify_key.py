# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors modify key."""

import selectors
import socket


sel = selectors.DefaultSelector()

try:
    a, b = socket.socketpair()
except AttributeError:
    print("no_socketpair")
else:
    sel.register(a, selectors.EVENT_READ)
    sel.modify(a, selectors.EVENT_WRITE)
    key = sel.get_key(a)
    print(bool(key.events & selectors.EVENT_WRITE))
    sel.unregister(a)
    a.close()
    b.close()
