"""select module shim for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement poll/epoll/kqueue/devpoll APIs and error mapping parity.

from __future__ import annotations

from typing import Any, Iterable

import selectors as _selectors
import time as _time
import molt.concurrency as _molt_concurrency

from _intrinsics import require_intrinsic as _require_intrinsic


__all__ = ["error", "select"]

error = OSError

_MOLT_IO_WAIT_NEW = _require_intrinsic("molt_io_wait_new", globals())
_MOLT_BLOCK_ON = _require_intrinsic("molt_block_on", globals())
_MOLT_ASYNC_SLEEP = _require_intrinsic("molt_async_sleep", globals())


def select(
    rlist: Iterable[Any],
    wlist: Iterable[Any],
    xlist: Iterable[Any],
    timeout: float | None = None,
) -> tuple[list[Any], list[Any], list[Any]]:
    io_wait = _MOLT_IO_WAIT_NEW
    block_on = _MOLT_BLOCK_ON
    to_handle = getattr(_selectors, "_fileobj_to_handle", None)
    if to_handle is None:
        raise RuntimeError("selectors._fileobj_to_handle unavailable")

    def _deadline_from_timeout(value: float | None):
        if value is None:
            return None
        if value <= 0:
            return _time.monotonic()
        return _time.monotonic() + value

    async def _wait_ready() -> tuple[list[Any], list[Any], list[Any]]:
        deadline = _deadline_from_timeout(timeout)
        chan = _molt_concurrency.channel()
        futures: list[object] = []

        async def _wait_one(obj: Any, fut: object) -> None:
            try:
                mask = int(await fut)
            except TimeoutError:
                mask = 0
            try:
                await chan.send_async((obj, mask))
            except Exception:
                pass

        for obj in rlist:
            fut = io_wait(to_handle(obj), _selectors.EVENT_READ, deadline)
            futures.append(fut)
            _molt_concurrency.spawn(_wait_one(obj, fut))
        for obj in wlist:
            fut = io_wait(to_handle(obj), _selectors.EVENT_WRITE, deadline)
            futures.append(fut)
            _molt_concurrency.spawn(_wait_one(obj, fut))
        for obj in xlist:
            fut = io_wait(
                to_handle(obj),
                _selectors.EVENT_READ | _selectors.EVENT_WRITE,
                deadline,
            )
            futures.append(fut)
            _molt_concurrency.spawn(_wait_one(obj, fut))
        if not futures:
            if timeout is None:
                return [], [], []
            if timeout > 0:
                await _MOLT_ASYNC_SLEEP(timeout, None)
            return [], [], []
        ready_r: list[Any] = []
        ready_w: list[Any] = []
        ready_x: list[Any] = []

        def _apply_ready(obj: Any, mask: int) -> None:
            if mask & _selectors.EVENT_READ:
                ready_r.append(obj)
            if mask & _selectors.EVENT_WRITE:
                ready_w.append(obj)
            if mask == 0:
                ready_x.append(obj)

        obj, mask = await chan.recv_async()
        _apply_ready(obj, mask)
        while True:
            ok, payload = chan.try_recv()
            if not ok:
                break
            obj, mask = payload
            _apply_ready(obj, mask)

        for fut in futures:
            cancel = getattr(fut, "cancel", None)
            if callable(cancel):
                try:
                    cancel()
                except Exception:
                    pass
        try:
            chan.close()
        except Exception:
            pass
        return ready_r, ready_w, ready_x

    return block_on(_wait_ready())
