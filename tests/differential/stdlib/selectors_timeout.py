# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors timeout."""

import selectors
import socket


sel = selectors.DefaultSelector()
print(sel.select(timeout=0.0) == [])
print(sel.select(timeout=-1.0) == [])

try:
    left, right = socket.socketpair()
except AttributeError:
    print("no_socketpair")
else:
    left.setblocking(False)
    right.setblocking(False)
    try:
        sel.register(left, selectors.EVENT_READ)
        print(sel.select(timeout=0.0) == [])

        right.sendall(b"x")
        events = sel.select(timeout=1.0)
        read_masks = [mask for key, mask in events if key.fileobj is left]
        print(len(read_masks) == 1)
        if read_masks:
            mask = read_masks[0]
            print(
                bool(mask & selectors.EVENT_READ),
                not bool(mask & selectors.EVENT_WRITE),
            )
        sel.unregister(left)
    finally:
        left.close()
        right.close()

sel.close()
