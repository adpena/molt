"""select module shim for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement poll/epoll/kqueue/devpoll APIs and error mapping parity.

from __future__ import annotations

from typing import Any, Iterable

import asyncio as _asyncio
import selectors as _selectors

__all__ = ["error", "select"]

error = OSError


def select(
    rlist: Iterable[Any],
    wlist: Iterable[Any],
    xlist: Iterable[Any],
    timeout: float | None = None,
) -> tuple[list[Any], list[Any], list[Any]]:
    io_wait = getattr(_selectors, "_molt_io_wait_new", None)
    block_on = getattr(_selectors, "_molt_block_on", None)
    to_handle = getattr(_selectors, "_fileobj_to_handle", None)
    if io_wait is not None and block_on is not None and to_handle is not None:

        async def _wait_ready() -> tuple[list[Any], list[Any], list[Any]]:
            ensure_future = getattr(_asyncio, "ensure_future", None)
            futures: list[tuple[Any, Any]] = []
            for obj in rlist:
                fut = io_wait(to_handle(obj), _selectors.EVENT_READ, None)
                if ensure_future is not None:
                    fut = ensure_future(fut)
                futures.append((obj, fut))
            for obj in wlist:
                fut = io_wait(to_handle(obj), _selectors.EVENT_WRITE, None)
                if ensure_future is not None:
                    fut = ensure_future(fut)
                futures.append((obj, fut))
            for obj in xlist:
                fut = io_wait(
                    to_handle(obj),
                    _selectors.EVENT_READ | _selectors.EVENT_WRITE,
                    None,
                )
                if ensure_future is not None:
                    fut = ensure_future(fut)
                futures.append((obj, fut))
            if not futures:
                if timeout is None:
                    return [], [], []
                if timeout > 0:
                    await _asyncio.sleep(timeout)
                return [], [], []
            done, pending = await _asyncio.wait(
                [f for _, f in futures],
                timeout=timeout,
                return_when=_asyncio.FIRST_COMPLETED,
            )
            ready_r: list[Any] = []
            ready_w: list[Any] = []
            ready_x: list[Any] = []
            for obj, fut in futures:
                if fut not in done:
                    continue
                try:
                    mask = int(fut.result())
                except TimeoutError:
                    mask = 0
                if mask & _selectors.EVENT_READ:
                    ready_r.append(obj)
                if mask & _selectors.EVENT_WRITE:
                    ready_w.append(obj)
                if mask == 0:
                    ready_x.append(obj)
            for fut in pending:
                try:
                    fut.cancel()
                except Exception:
                    pass
            return ready_r, ready_w, ready_x

        running = getattr(_asyncio, "_get_running_loop", None)
        set_running = getattr(_asyncio, "_set_running_loop", None)
        temp_loop = None
        if running is not None and set_running is not None:
            if running() is None:
                temp_loop = _asyncio.new_event_loop()
                _asyncio.set_event_loop(temp_loop)
                set_running(temp_loop)
        try:
            return block_on(_wait_ready())
        finally:
            if temp_loop is not None and set_running is not None:
                set_running(None)
                _asyncio.set_event_loop(None)
                try:
                    temp_loop.close()
                except Exception:
                    pass

    # TODO(perf, owner:stdlib, milestone:SL2, priority:P3, status:planned): reuse selectors across calls to reduce register/unregister churn.
    selector = _selectors.DefaultSelector()
    key_map: dict[int, Any] = {}

    def _register(obj: Any, events: int) -> None:
        try:
            key = selector.get_key(obj)
            selector.modify(obj, key.events | events)
            key_map[key.fd] = obj
        except KeyError:
            key = selector.register(obj, events)
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
