"""Intrinsic-backed queue primitives for Molt."""

from __future__ import annotations

from typing import Any

import sys as _sys

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_QUEUE_NEW = _require_intrinsic("molt_queue_new", globals())
_MOLT_QUEUE_LIFO_NEW = _require_intrinsic("molt_queue_lifo_new", globals())
_MOLT_QUEUE_PRIORITY_NEW = _require_intrinsic("molt_queue_priority_new", globals())
_MOLT_QUEUE_QSIZE = _require_intrinsic("molt_queue_qsize", globals())
_MOLT_QUEUE_EMPTY = _require_intrinsic("molt_queue_empty", globals())
_MOLT_QUEUE_FULL = _require_intrinsic("molt_queue_full", globals())
_MOLT_QUEUE_PUT = _require_intrinsic("molt_queue_put", globals())
_MOLT_QUEUE_GET = _require_intrinsic("molt_queue_get", globals())
_MOLT_QUEUE_TASK_DONE = _require_intrinsic("molt_queue_task_done", globals())
_MOLT_QUEUE_JOIN = _require_intrinsic("molt_queue_join", globals())
_MOLT_QUEUE_DROP = _require_intrinsic("molt_queue_drop", globals())
_MOLT_MODULE_CACHE_SET = _require_intrinsic("molt_module_cache_set", globals())

_GET_TIMEOUT = object()


__all__ = [
    "Empty",
    "Full",
    "Queue",
    "SimpleQueue",
    "LifoQueue",
    "PriorityQueue",
]


class Empty(Exception):
    pass


class Full(Exception):
    pass


def _normalize_timeout(
    block: bool, timeout: float | None, *, op_name: str
) -> float | None:
    if timeout is None:
        return None
    if not block:
        raise ValueError(f"can't specify a timeout for a non-blocking {op_name}")
    value = float(timeout)
    if value < 0.0:
        raise ValueError("'timeout' must be a non-negative number")
    return value


class Queue:
    def __init__(self, maxsize: int = 0) -> None:
        self.maxsize = int(maxsize)
        self._handle = _MOLT_QUEUE_NEW(self.maxsize)

    @classmethod
    def __class_getitem__(cls, _item: Any) -> type["Queue"]:
        return cls

    def qsize(self) -> int:
        return int(_MOLT_QUEUE_QSIZE(self._handle))

    def empty(self) -> bool:
        return bool(_MOLT_QUEUE_EMPTY(self._handle))

    def full(self) -> bool:
        return bool(_MOLT_QUEUE_FULL(self._handle))

    def put(self, item: Any, block: bool = True, timeout: float | None = None) -> None:
        wait = _normalize_timeout(bool(block), timeout, op_name="put")
        ok = bool(_MOLT_QUEUE_PUT(self._handle, item, bool(block), wait))
        if not ok:
            raise Full

    def put_nowait(self, item: Any) -> None:
        self.put(item, block=False)

    def get(self, block: bool = True, timeout: float | None = None) -> Any:
        wait = _normalize_timeout(bool(block), timeout, op_name="get")
        item = _MOLT_QUEUE_GET(self._handle, bool(block), wait, _GET_TIMEOUT)
        if item is _GET_TIMEOUT:
            raise Empty
        return item

    def get_nowait(self) -> Any:
        return self.get(block=False)

    def task_done(self) -> None:
        if not bool(_MOLT_QUEUE_TASK_DONE(self._handle)):
            raise ValueError("task_done() called too many times")

    def join(self) -> None:
        _MOLT_QUEUE_JOIN(self._handle)

    def __del__(self) -> None:
        try:
            _MOLT_QUEUE_DROP(self._handle)
        except Exception:
            return


class SimpleQueue:
    def __init__(self) -> None:
        self._handle = _MOLT_QUEUE_NEW(0)

    @classmethod
    def __class_getitem__(cls, _item: Any) -> type["SimpleQueue"]:
        return cls

    def qsize(self) -> int:
        return int(_MOLT_QUEUE_QSIZE(self._handle))

    def empty(self) -> bool:
        return bool(_MOLT_QUEUE_EMPTY(self._handle))

    def put(self, item: Any, block: bool = True, timeout: float | None = None) -> None:
        # CPython SimpleQueue ignores block/timeout; keep the signature compatible.
        _ = block
        _ = timeout
        ok = bool(_MOLT_QUEUE_PUT(self._handle, item, True, None))
        if not ok:
            raise Full

    def put_nowait(self, item: Any) -> None:
        self.put(item)

    def get(self, block: bool = True, timeout: float | None = None) -> Any:
        wait = _normalize_timeout(bool(block), timeout, op_name="get")
        item = _MOLT_QUEUE_GET(self._handle, bool(block), wait, _GET_TIMEOUT)
        if item is _GET_TIMEOUT:
            raise Empty
        return item

    def get_nowait(self) -> Any:
        return self.get(block=False)

    def __del__(self) -> None:
        try:
            _MOLT_QUEUE_DROP(self._handle)
        except Exception:
            return


class LifoQueue(Queue):
    def __init__(self, maxsize: int = 0) -> None:
        self.maxsize = int(maxsize)
        self._handle = _MOLT_QUEUE_LIFO_NEW(self.maxsize)


class PriorityQueue(Queue):
    def __init__(self, maxsize: int = 0) -> None:
        self.maxsize = int(maxsize)
        self._handle = _MOLT_QUEUE_PRIORITY_NEW(self.maxsize)


_module = _sys.modules.get(__name__)
if _module is not None:
    _MOLT_MODULE_CACHE_SET(__name__, _module)
    if __name__ != "queue":
        _MOLT_MODULE_CACHE_SET("queue", _module)
