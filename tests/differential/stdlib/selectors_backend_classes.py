# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors backend classes."""

import selectors
import socket


def _probe(name: str, selector_cls) -> tuple[str, object]:
    if selector_cls is None:
        return (name, "missing")
    left, right = socket.socketpair()
    selector = selector_cls()
    try:
        selector.register(left, selectors.EVENT_READ)
        right.send(b"x")
        ready = selector.select(1.0)
        ok = any(
            int(key.fd) == int(left.fileno()) and bool(mask & selectors.EVENT_READ)
            for key, mask in ready
        )
        return (name, ok)
    except NotImplementedError:
        return (name, "unsupported")
    finally:
        selector.close()
        left.close()
        right.close()


print(
    [
        _probe("poll", getattr(selectors, "PollSelector", None)),
        _probe("epoll", getattr(selectors, "EpollSelector", None)),
        _probe("devpoll", getattr(selectors, "DevpollSelector", None)),
        _probe("kqueue", getattr(selectors, "KqueueSelector", None)),
    ]
)
