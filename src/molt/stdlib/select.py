"""select module shim for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement poll/epoll/kqueue/devpoll APIs and error mapping parity.

from __future__ import annotations

from typing import Any, Iterable

import selectors as _selectors

__all__ = ["error", "select"]

error = OSError


def select(
    rlist: Iterable[Any],
    wlist: Iterable[Any],
    xlist: Iterable[Any],
    timeout: float | None = None,
) -> tuple[list[Any], list[Any], list[Any]]:
    # TODO(perf, owner:stdlib, milestone:SL2, priority:P3, status:planned): reuse selectors across calls to reduce register/unregister churn.
    selector = _selectors.DefaultSelector()
    key_map: dict[int, Any] = {}

    def _register(obj: Any, events: int) -> None:
        try:
            key = selector.get_key(obj)
            selector.modify(obj, key.events | events, key.data)
            key_map[key.fd] = obj
        except KeyError:
            key = selector.register(obj, events, None)
            key_map[key.fd] = obj

    for obj in rlist:
        _register(obj, _selectors.EVENT_READ)
    for obj in wlist:
        _register(obj, _selectors.EVENT_WRITE)
    for obj in xlist:
        _register(obj, _selectors.EVENT_READ | _selectors.EVENT_WRITE)

    try:
        ready = selector.select(timeout)
    finally:
        selector.close()

    ready_r: list[Any] = []
    ready_w: list[Any] = []
    ready_x: list[Any] = []
    for key, mask in ready:
        if mask & _selectors.EVENT_READ:
            ready_r.append(key.fileobj)
        if mask & _selectors.EVENT_WRITE:
            ready_w.append(key.fileobj)
        if mask == 0:
            ready_x.append(key.fileobj)

    return ready_r, ready_w, ready_x
