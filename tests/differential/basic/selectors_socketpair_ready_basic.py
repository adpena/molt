"""Purpose: differential coverage for selectors readiness on socketpair."""

# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
import selectors
import socket

sel = selectors.DefaultSelector()
left, right = socket.socketpair()
left.setblocking(False)
right.setblocking(False)

try:
    sel.register(left, selectors.EVENT_WRITE, data="left")
    sel.register(right, selectors.EVENT_READ, data="right")

    events = sel.select(timeout=0)
    write_ready = any(
        key.fileobj is left and (mask & selectors.EVENT_WRITE)
        for key, mask in events
    )
    print("write_ready", bool(write_ready))

    left.send(b"hi")
    events = sel.select(timeout=1)
    read_ready = any(
        key.fileobj is right and (mask & selectors.EVENT_READ)
        for key, mask in events
    )
    print("read_ready", bool(read_ready))

    data = right.recv(2)
    print(data)

    events = sel.select(timeout=0)
    read_after = any(
        key.fileobj is right and (mask & selectors.EVENT_READ)
        for key, mask in events
    )
    print("read_ready_after", bool(read_after))
finally:
    sel.close()
    left.close()
    right.close()
