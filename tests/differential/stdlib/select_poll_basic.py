# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for select poll basic."""

import select
import socket


if not hasattr(select, "poll"):
    print("no_poll")
else:
    try:
        a, b = socket.socketpair()
    except AttributeError:
        print("no_socketpair")
    else:
        poller = select.poll()
        poller.register(a, select.POLLIN)
        b.send(b"x")
        events = poller.poll(1000)
        has_event = bool(events)
        mask = events[0][1] if has_event else 0
        print(has_event, bool(mask & select.POLLIN))
        a.close()
        b.close()
