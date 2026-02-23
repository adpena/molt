# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors modify key."""

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
        key = sel.register(left, selectors.EVENT_READ, data="initial")
        print(key.data, bool(key.events & selectors.EVENT_READ))

        key = sel.modify(left, selectors.EVENT_WRITE, data="writer")
        print(
            key.data,
            bool(key.events & selectors.EVENT_WRITE),
            not bool(key.events & selectors.EVENT_READ),
        )

        write_ready = any(
            key.fileobj is left
            and bool(mask & selectors.EVENT_WRITE)
            and not bool(mask & selectors.EVENT_READ)
            for key, mask in sel.select(timeout=1.0)
        )
        print(write_ready)

        key = sel.modify(left, selectors.EVENT_READ, data="reader")
        print(
            key.data,
            bool(key.events & selectors.EVENT_READ),
            not bool(key.events & selectors.EVENT_WRITE),
        )

        right.sendall(b"x")
        read_ready = any(
            key.fileobj is left
            and bool(mask & selectors.EVENT_READ)
            and not bool(mask & selectors.EVENT_WRITE)
            for key, mask in sel.select(timeout=1.0)
        )
        print(read_ready)

        key = sel.get_key(left)
        print(key.data, bool(key.events & selectors.EVENT_READ))
        sel.unregister(left)
    finally:
        sel.close()
        left.close()
        right.close()
