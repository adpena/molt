"""Intrinsic-backed queue primitives for Molt."""

from __future__ import annotations

from typing import Any

import sys as _sys

from _intrinsics import require_intrinsic as _require_intrinsic
from _queue import Empty, SimpleQueue

_PY_MAJOR = int(_sys.version_info[0])
_PY_MINOR = int(_sys.version_info[1])
_PY_GE_313 = _PY_MAJOR > 3 or (_PY_MAJOR == 3 and _PY_MINOR >= 13)

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
if _PY_GE_313:
    _MOLT_QUEUE_SHUTDOWN = _require_intrinsic("molt_queue_shutdown", globals())
    _MOLT_QUEUE_IS_SHUTDOWN = _require_intrinsic("molt_queue_is_shutdown", globals())

_GET_TIMEOUT = object()


__all__ = [
    "Empty",
    "Full",
    "Queue",
    "PriorityQueue",
    "LifoQueue",
    "SimpleQueue",
]
if _PY_GE_313:
    __all__.insert(2, "ShutDown")


class Full(Exception):
    pass


def _queue_shutdown(handle: object, immediate: object) -> None:
    _MOLT_QUEUE_SHUTDOWN(handle, bool(immediate))


def _queue_is_shutdown(handle: object) -> bool:
    if not _PY_GE_313:
        return False
    return bool(_MOLT_QUEUE_IS_SHUTDOWN(handle))


if _PY_GE_313:

    class ShutDown(Exception):
        pass


def _normalize_get_timeout(block: bool, timeout: float | None) -> float | None:
    # CPython ignores timeout for non-blocking Queue.get calls.
    if not block or timeout is None:
        return None
    if timeout < 0:
        raise ValueError("'timeout' must be a non-negative number")
    return timeout


def _normalize_put_timeout(
    maxsize: int, block: bool, timeout: float | None
) -> float | None:
    # CPython ignores timeout for non-blocking Queue.put calls and unbounded queues.
    if maxsize <= 0 or not block or timeout is None:
        return None
    if timeout < 0:
        raise ValueError("'timeout' must be a non-negative number")
    return timeout


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
        blocking = bool(block)
        wait = _normalize_put_timeout(self.maxsize, blocking, timeout)
        ok = bool(_MOLT_QUEUE_PUT(self._handle, item, blocking, wait))
        if not ok:
            if _queue_is_shutdown(self._handle):
                raise ShutDown
            raise Full

    def put_nowait(self, item: Any) -> None:
        self.put(item, block=False)

    def get(self, block: bool = True, timeout: float | None = None) -> Any:
        blocking = bool(block)
        wait = _normalize_get_timeout(blocking, timeout)
        item = _MOLT_QUEUE_GET(self._handle, blocking, wait, _GET_TIMEOUT)
        if item is _GET_TIMEOUT:
            if _queue_is_shutdown(self._handle):
                raise ShutDown
            raise Empty
        return item

    def get_nowait(self) -> Any:
        return self.get(block=False)

    def task_done(self) -> None:
        if not bool(_MOLT_QUEUE_TASK_DONE(self._handle)):
            raise ValueError("task_done() called too many times")

    def join(self) -> None:
        _MOLT_QUEUE_JOIN(self._handle)

    def shutdown(self, immediate: bool = False) -> None:
        _queue_shutdown(self._handle, immediate)

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


if not _PY_GE_313:
    del Queue.shutdown


_module = _sys.modules.get(__name__)
if _module is not None:
    _MOLT_MODULE_CACHE_SET(__name__, _module)
    if __name__ != "queue":
        _MOLT_MODULE_CACHE_SET("queue", _module)
