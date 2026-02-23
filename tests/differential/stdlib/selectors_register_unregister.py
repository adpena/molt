# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors register unregister."""

import selectors
import socket


sel = selectors.DefaultSelector()

try:
    left, right = socket.socketpair()
except AttributeError:
    print("no_socketpair")
else:
    left.setblocking(False)
    right.setblocking(False)
    try:
        key = sel.register(left, selectors.EVENT_READ, data="left")
        print(key.fileobj is left, bool(key.events & selectors.EVENT_READ), key.data)

        try:
            sel.register(left, selectors.EVENT_READ)
        except Exception as exc:
            print(type(exc).__name__)

        removed = sel.unregister(left)
        print(removed.fileobj is left, removed.data)

        right.sendall(b"x")
        print(sel.select(timeout=0.0) == [])
    finally:
        sel.close()
        left.close()
        right.close()
