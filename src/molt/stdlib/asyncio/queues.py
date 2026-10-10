"""Queue authority for `asyncio.queues`."""

from __future__ import annotations

import collections
from collections import deque as _deque
import heapq as _heapq
import heapq
import sys as _sys
import types as _types
from typing import Any


_VERSION_INFO = getattr(_sys, "version_info", (3, 12, 0, "final", 0))
_EXPOSE_QUEUE_SHUTDOWN = _VERSION_INFO >= (3, 13)

from asyncio import (
    Event,
    Future,
    _asyncio_waiters_remove,
    _is_cancelled_exc,
    _require_asyncio_intrinsic,
    _molt_asyncio_queue_drop,
    _molt_asyncio_queue_empty,
    _molt_asyncio_queue_full,
    _molt_asyncio_queue_get_nowait,
    _molt_asyncio_queue_is_shutdown,
    _molt_asyncio_queue_maxsize,
    _molt_asyncio_queue_new,
    _molt_asyncio_queue_put_nowait,
    _molt_asyncio_queue_qsize,
    _molt_asyncio_queue_shutdown,
    _molt_asyncio_queue_task_done,
    _molt_asyncio_queue_unfinished_tasks,
    _molt_generic_alias_new,
)

GenericAlias = _types.GenericAlias
locks: Any | None = None
mixins: Any | None = None

class QueueEmpty(Exception):
    pass


class QueueFull(Exception):
    pass


class _QueueShutDown(Exception):
    pass


if _EXPOSE_QUEUE_SHUTDOWN:
    QueueShutDown = _QueueShutDown

class Queue:
    _Q_TYPE: int = 0  # FIFO

    def __init__(self, maxsize: int = 0) -> None:
        if maxsize < 0:
            raise ValueError("maxsize must be >= 0")
        self._maxsize = maxsize
        self._q_handle: int = _molt_asyncio_queue_new(maxsize, self._Q_TYPE)
        self._getters: _deque[Future] = _deque()
        self._putters: _deque[Future] = _deque()
        self._finished = Event()
        self._finished.set()
        self._shutdown = False
        self._init()

    @property
    def maxsize(self) -> int:
        return self._maxsize

    def _init(self) -> None:
        self._queue: Any = _deque()

    def qsize(self) -> int:
        return int(_molt_asyncio_queue_qsize(self._q_handle))

    def _handle_maxsize(self) -> int:
        return int(_molt_asyncio_queue_maxsize(self._q_handle))

    def empty(self) -> bool:
        return bool(_molt_asyncio_queue_empty(self._q_handle))

    def full(self) -> bool:
        return bool(_molt_asyncio_queue_full(self._q_handle))

    async def put(self, item: Any) -> None:
        if _molt_asyncio_queue_is_shutdown(self._q_handle):
            raise _QueueShutDown
        while self.full():
            fut = Future()
            self._putters.append(fut)
            try:
                await fut
            except BaseException as exc:
                if _is_cancelled_exc(exc):
                    _asyncio_waiters_remove(self._putters, fut)
                raise
            if _molt_asyncio_queue_is_shutdown(self._q_handle):
                raise _QueueShutDown
        self._put_nowait(item)

    def put_nowait(self, item: Any) -> None:
        if _molt_asyncio_queue_is_shutdown(self._q_handle):
            raise _QueueShutDown
        if self.full():
            raise QueueFull
        self._put_nowait(item)

    def _put_nowait(self, item: Any) -> None:
        _molt_asyncio_queue_put_nowait(self._q_handle, item)
        if self._finished.is_set():
            self._finished.clear()
        if self._getters:
            delivered = _molt_asyncio_queue_get_nowait(self._q_handle)
            if not self._wakeup_next(self._getters, delivered):
                self._put(delivered)
        else:
            self._put(item)

    def _put(self, item: Any) -> None:
        self._queue.append(item)

    async def get(self) -> Any:
        if not _molt_asyncio_queue_empty(self._q_handle):
            return self._get_nowait()
        if _molt_asyncio_queue_is_shutdown(self._q_handle):
            raise _QueueShutDown
        fut = Future()
        self._getters.append(fut)
        try:
            return await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                _asyncio_waiters_remove(self._getters, fut)
            raise

    def get_nowait(self) -> Any:
        if not _molt_asyncio_queue_empty(self._q_handle):
            return self._get_nowait()
        if _molt_asyncio_queue_is_shutdown(self._q_handle):
            raise _QueueShutDown
        raise QueueEmpty

    def _get_nowait(self) -> Any:
        item = self._get()
        _molt_asyncio_queue_get_nowait(self._q_handle)
        if self._putters:
            self._wakeup_next(self._putters, None)
        return item

    def _get(self) -> Any:
        return self._queue.popleft()

    def _wakeup_next(self, waiters: Any, result: Any) -> bool:
        while waiters:
            fut = waiters.popleft()
            if not fut.done():
                fut.set_result(result)
                return True
        return False

    def _wakeup_all_exception(self, waiters: Any, exc: BaseException) -> None:
        while waiters:
            fut = waiters.popleft()
            if not fut.done():
                fut.set_exception(exc)

    def task_done(self) -> None:
        _molt_asyncio_queue_task_done(self._q_handle)
        if int(_molt_asyncio_queue_unfinished_tasks(self._q_handle)) == 0:
            self._finished.set()

    async def join(self) -> None:
        await self._finished.wait()

    if _EXPOSE_QUEUE_SHUTDOWN:

        def shutdown(self) -> None:
            self._shutdown = True
            _molt_asyncio_queue_shutdown(self._q_handle, False)
            exc = _QueueShutDown()
            self._wakeup_all_exception(self._getters, exc)
            self._wakeup_all_exception(self._putters, exc)

    def __repr__(self) -> str:
        size = self.qsize()
        maxsize = self._maxsize
        if maxsize > 0:
            return f"<Queue maxsize={maxsize} qsize={size}>"
        return f"<Queue qsize={size}>"

    @classmethod
    def __class_getitem__(cls, item: Any) -> Any:
        return _require_asyncio_intrinsic(_molt_generic_alias_new, "generic_alias_new")(
            cls, item
        )

    def __del__(self) -> None:
        handle = getattr(self, "_q_handle", None)
        if handle is not None:
            _molt_asyncio_queue_drop(handle)


class PriorityQueue(Queue):
    _Q_TYPE: int = 2  # Priority

    def _init(self) -> None:
        self._queue = []

    def _put(self, item: Any) -> None:
        _heapq.heappush(self._queue, item)

    def _get(self) -> Any:
        return _heapq.heappop(self._queue)


class LifoQueue(Queue):
    _Q_TYPE: int = 1  # LIFO

    def _init(self) -> None:
        self._queue = []

    def _put(self, item: Any) -> None:
        self._queue.append(item)

    def _get(self) -> Any:
        return self._queue.pop()

__all__ = [
    "GenericAlias",
    "LifoQueue",
    "PriorityQueue",
    "Queue",
    "QueueEmpty",
    "QueueFull",
    "collections",
    "heapq",
    "locks",
    "mixins",
]
if _EXPOSE_QUEUE_SHUTDOWN:
    __all__.append("QueueShutDown")
