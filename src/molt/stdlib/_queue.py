"""Intrinsic-backed queue core primitives for Molt."""

from __future__ import annotations

from typing import Any as _Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_QUEUE_NEW = _require_intrinsic("molt_queue_new", globals())
_MOLT_QUEUE_QSIZE = _require_intrinsic("molt_queue_qsize", globals())
_MOLT_QUEUE_EMPTY = _require_intrinsic("molt_queue_empty", globals())
_MOLT_QUEUE_PUT = _require_intrinsic("molt_queue_put", globals())
_MOLT_QUEUE_GET = _require_intrinsic("molt_queue_get", globals())
_MOLT_QUEUE_DROP = _require_intrinsic("molt_queue_drop", globals())

_GET_TIMEOUT = object()


class Empty(Exception):
    pass


def _normalize_get_timeout(block: bool, timeout: float | None) -> float | None:
    if not block:
        return None
    return timeout


class SimpleQueue:
    def __init__(self) -> None:
        self._handle = _MOLT_QUEUE_NEW(0)

    @classmethod
    def __class_getitem__(cls, _item: _Any) -> type["SimpleQueue"]:
        return cls

    def qsize(self) -> int:
        return int(_MOLT_QUEUE_QSIZE(self._handle))

    def empty(self) -> bool:
        return bool(_MOLT_QUEUE_EMPTY(self._handle))

    def put(self, item: _Any, block: bool = True, timeout: float | None = None) -> None:
        # CPython SimpleQueue ignores block/timeout; keep the signature compatible.
        _ = block
        _ = timeout
        ok = bool(_MOLT_QUEUE_PUT(self._handle, item, True, None))
        if not ok:
            raise RuntimeError("SimpleQueue put intrinsic unexpectedly returned False")

    def put_nowait(self, item: _Any) -> None:
        self.put(item)

    def get(self, block: bool = True, timeout: float | None = None) -> _Any:
        blocking = bool(block)
        wait = _normalize_get_timeout(blocking, timeout)
        item = _MOLT_QUEUE_GET(self._handle, blocking, wait, _GET_TIMEOUT)
        if item is _GET_TIMEOUT:
            raise Empty
        return item

    def get_nowait(self) -> _Any:
        return self.get(block=False)

    def __del__(self) -> None:
        try:
            _MOLT_QUEUE_DROP(self._handle)
        except Exception:
            return
