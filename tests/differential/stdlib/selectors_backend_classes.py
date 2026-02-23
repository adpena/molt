# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors backend/default behavior."""

import selectors
import socket


def _probe_selector(selector) -> object:
    try:
        left, right = socket.socketpair()
    except AttributeError:
        selector.close()
        return "no_socketpair"

    left.setblocking(False)
    right.setblocking(False)
    try:
        selector.register(left, selectors.EVENT_READ)
        right.sendall(b"x")
        ready = selector.select(1.0)
        return any(
            key.fileobj is left
            and bool(mask & selectors.EVENT_READ)
            and not bool(mask & selectors.EVENT_WRITE)
            for key, mask in ready
        )
    except NotImplementedError:
        return "unsupported"
    finally:
        try:
            selector.unregister(left)
        except Exception:
            pass
        selector.close()
        left.close()
        right.close()


def _probe(name: str, selector_cls) -> tuple[str, object]:
    if selector_cls is None:
        return (name, "missing")
    return (name, _probe_selector(selector_cls()))


backend_results = [
    _probe("select", getattr(selectors, "SelectSelector", None)),
    _probe("poll", getattr(selectors, "PollSelector", None)),
    _probe("epoll", getattr(selectors, "EpollSelector", None)),
    _probe("devpoll", getattr(selectors, "DevpollSelector", None)),
    _probe("kqueue", getattr(selectors, "KqueueSelector", None)),
]
supported_backends = {name for name, result in backend_results if result is True}

default_selector = selectors.DefaultSelector()
default_name = type(default_selector).__name__
default_result = _probe_selector(default_selector)
class_to_backend = {
    "SelectSelector": "select",
    "PollSelector": "poll",
    "EpollSelector": "epoll",
    "DevpollSelector": "devpoll",
    "KqueueSelector": "kqueue",
}
default_backend = class_to_backend.get(default_name, default_name)
default_in_supported = (
    default_result == "no_socketpair" or default_backend in supported_backends
)

print(backend_results)
print(("default", default_name, default_result, default_in_supported))
