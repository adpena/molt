# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for select poll/epoll/kqueue/devpoll objects."""

import select
import socket


def _probe_poll() -> tuple[str, object]:
    if not hasattr(select, "poll"):
        return ("poll", "missing")
    left, right = socket.socketpair()
    poller = select.poll()
    try:
        poller.register(left, select.POLLIN)
        right.send(b"x")
        events = poller.poll(1000)
        ok = bool(events) and bool(events[0][1] & select.POLLIN)
        return ("poll", ok)
    finally:
        poller.close()
        left.close()
        right.close()


def _probe_epoll() -> tuple[str, object]:
    if not hasattr(select, "epoll"):
        return ("epoll", "missing")
    left, right = socket.socketpair()
    poller = select.epoll()
    try:
        poller.register(left.fileno(), select.EPOLLIN)
        right.send(b"x")
        events = poller.poll(1.0)
        ok = bool(events) and bool(events[0][1] & select.EPOLLIN)
        return ("epoll", ok)
    finally:
        poller.close()
        left.close()
        right.close()


def _probe_devpoll() -> tuple[str, object]:
    if not hasattr(select, "devpoll"):
        return ("devpoll", "missing")
    left, right = socket.socketpair()
    poller = select.devpoll()
    try:
        poller.register(left, select.POLLIN)
        right.send(b"x")
        events = poller.poll(1000)
        ok = bool(events) and bool(events[0][1] & select.POLLIN)
        return ("devpoll", ok)
    finally:
        poller.close()
        left.close()
        right.close()


def _probe_kqueue() -> tuple[str, object]:
    if not (hasattr(select, "kqueue") and hasattr(select, "kevent")):
        return ("kqueue", "missing")
    left, right = socket.socketpair()
    poller = select.kqueue()
    try:
        add = select.kevent(left.fileno(), select.KQ_FILTER_READ, select.KQ_EV_ADD)
        poller.control([add], 0, 0)
        right.send(b"x")
        events = poller.control(None, 1, 1.0)
        ok = bool(events) and int(events[0].filter) == int(select.KQ_FILTER_READ)
        return ("kqueue", ok)
    finally:
        poller.close()
        left.close()
        right.close()


print([_probe_poll(), _probe_epoll(), _probe_devpoll(), _probe_kqueue()])
