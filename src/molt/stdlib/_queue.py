"""Intrinsic-backed queue core primitives for Molt."""

from __future__ import annotations

from typing import Any as _Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_QUEUE_NEW = _require_intrinsic("molt_queue_new")
_MOLT_QUEUE_QSIZE = _require_intrinsic("molt_queue_qsize")
_MOLT_QUEUE_EMPTY = _require_intrinsic("molt_queue_empty")
_MOLT_QUEUE_PUT = _require_intrinsic("molt_queue_put")
_MOLT_QUEUE_GET = _require_intrinsic("molt_queue_get")
_MOLT_QUEUE_DROP = _require_intrinsic("molt_queue_drop")

_GET_TIMEOUT = object()


class Empty(Exception):
    pass


def _normalize_get_timeout(block: bool, timeout: float | None) -> float | None:
    if not block:
        return None
    return timeout


class SimpleQueue:
    def __init__(self, _queue_new=_MOLT_QUEUE_NEW) -> None:
        self._handle = _queue_new(0)

    @classmethod
    def __class_getitem__(cls, _item: _Any) -> type["SimpleQueue"]:
        return cls

    def qsize(self, _queue_qsize=_MOLT_QUEUE_QSIZE) -> int:
        return int(_queue_qsize(self._handle))

    def empty(self, _queue_empty=_MOLT_QUEUE_EMPTY) -> bool:
        return bool(_queue_empty(self._handle))

    def put(
        self,
        item: _Any,
        block: bool = True,
        timeout: float | None = None,
        _queue_put=_MOLT_QUEUE_PUT,
    ) -> None:
        # CPython SimpleQueue ignores block/timeout; keep the signature compatible.
        _ = block
        _ = timeout
        ok = bool(_queue_put(self._handle, item, True, None))
        if not ok:
            raise RuntimeError("SimpleQueue put intrinsic unexpectedly returned False")

    def put_nowait(self, item: _Any) -> None:
        self.put(item)

    def get(
        self,
        block: bool = True,
        timeout: float | None = None,
        _queue_get=_MOLT_QUEUE_GET,
    ) -> _Any:
        blocking = bool(block)
        wait = _normalize_get_timeout(blocking, timeout)
        item = _queue_get(self._handle, blocking, wait, _GET_TIMEOUT)
        if item is _GET_TIMEOUT:
            raise Empty
        return item

    def get_nowait(self) -> _Any:
        return self.get(block=False)

    def __del__(self, _queue_drop=_MOLT_QUEUE_DROP) -> None:
        try:
            _queue_drop(self._handle)
        except Exception:
            return


del _MOLT_QUEUE_NEW
del _MOLT_QUEUE_QSIZE
del _MOLT_QUEUE_EMPTY
del _MOLT_QUEUE_PUT
del _MOLT_QUEUE_GET
del _MOLT_QUEUE_DROP

globals().pop("_require_intrinsic", None)
