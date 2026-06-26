"""Synchronization primitive authority for `asyncio.locks`."""

from __future__ import annotations

import collections
from collections import deque as _deque
import enum
from typing import Any, Callable

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from asyncio import (
    BrokenBarrierError,
    Future,
    _asyncio_condition_wait_for_step,
    _asyncio_waiters_notify,
    _asyncio_waiters_remove,
    _current_token_id,
    _is_cancelled_exc,
    _register_event_waiter,
    molt_asyncio_event_clear_handle,
    molt_asyncio_event_drop,
    molt_asyncio_event_is_set,
    molt_asyncio_event_new,
    molt_asyncio_event_set_fast,
    molt_asyncio_event_set_waiters,
    molt_asyncio_lock_acquire_fast,
    molt_asyncio_lock_drop,
    molt_asyncio_lock_locked,
    molt_asyncio_lock_new,
    molt_asyncio_lock_release_fast,
    molt_asyncio_semaphore_acquire_fast,
    molt_asyncio_semaphore_drop,
    molt_asyncio_semaphore_new,
    molt_asyncio_semaphore_release_fast,
    molt_asyncio_semaphore_value,
    _require_asyncio_intrinsic,
    _unregister_event_waiter,
)

exceptions: Any | None = None
mixins: Any | None = None


class Event:
    def __init__(self) -> None:
        self._evt_handle: int = molt_asyncio_event_new()
        self._waiters: list[Future] = []

    def is_set(self) -> bool:
        return bool(molt_asyncio_event_is_set(self._evt_handle))

    def set(self) -> None:
        if molt_asyncio_event_is_set(self._evt_handle):
            return None
        molt_asyncio_event_set_fast(self._evt_handle)
        waiters = self._waiters
        self._waiters = []
        _require_asyncio_intrinsic(
            molt_asyncio_event_set_waiters, "asyncio_event_set_waiters"
        )(waiters, True)
        return None

    def clear(self) -> None:
        molt_asyncio_event_clear_handle(self._evt_handle)

    async def wait(self) -> bool:
        if molt_asyncio_event_is_set(self._evt_handle):
            return True
        fut = Future()
        fut._molt_event_owner = self
        token_id = _current_token_id()
        fut._molt_event_token_id = token_id
        self._waiters.append(fut)
        _register_event_waiter(token_id, fut)
        try:
            return await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                _unregister_event_waiter(token_id, fut)
                _asyncio_waiters_remove(self._waiters, fut)
            raise

    def __repr__(self) -> str:
        state = "set" if self.is_set() else "unset"
        return f"<Event [{state}]>"

    def __del__(self) -> None:
        handle = getattr(self, "_evt_handle", None)
        if handle is not None:
            molt_asyncio_event_drop(handle)


class Lock:
    def __init__(self) -> None:
        self._lock_handle: int = molt_asyncio_lock_new()
        self._waiters: _deque[Future] = _deque()

    def locked(self) -> bool:
        return bool(molt_asyncio_lock_locked(self._lock_handle))

    async def acquire(self) -> bool:
        if molt_asyncio_lock_acquire_fast(self._lock_handle):
            return True
        fut = Future()
        self._waiters.append(fut)
        try:
            await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                _asyncio_waiters_remove(self._waiters, fut)
            raise
        molt_asyncio_lock_acquire_fast(self._lock_handle)
        return True

    def release(self) -> None:
        if not molt_asyncio_lock_locked(self._lock_handle):
            raise RuntimeError("Lock is not acquired")
        molt_asyncio_lock_release_fast(self._lock_handle)
        if self._waiters:
            _asyncio_waiters_notify(self._waiters, 1, True)

    async def __aenter__(self) -> "Lock":
        await self.acquire()
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.release()

    def __repr__(self) -> str:
        state = "locked" if self.locked() else "unlocked"
        return f"<Lock [{state}]>"

    def __del__(self) -> None:
        handle = getattr(self, "_lock_handle", None)
        if handle is not None:
            molt_asyncio_lock_drop(handle)


class Condition:
    def __init__(self, lock: Lock | None = None) -> None:
        self._lock = lock or Lock()
        self._waiters: _deque[Future] = _deque()

    def locked(self) -> bool:
        return self._lock.locked()

    async def acquire(self) -> bool:
        return await self._lock.acquire()

    def release(self) -> None:
        self._lock.release()

    async def __aenter__(self) -> "Condition":
        await self.acquire()
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.release()

    async def wait(self) -> bool:
        if not self.locked():
            raise RuntimeError("Condition lock is not acquired")
        fut = Future()
        self._waiters.append(fut)
        self.release()
        try:
            await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                _asyncio_waiters_remove(self._waiters, fut)
            raise
        finally:
            await self.acquire()
        return True

    async def wait_for(self, predicate: Callable[[], bool]) -> bool:
        while True:
            done, payload = _asyncio_condition_wait_for_step(self, predicate)
            if done:
                return payload
            await payload

    def notify(self, n: int = 1) -> None:
        if not self.locked():
            raise RuntimeError("Condition lock is not acquired")
        _asyncio_waiters_notify(self._waiters, n, True)

    def notify_all(self) -> None:
        self.notify(len(self._waiters))

    def __repr__(self) -> str:
        state = "locked" if self.locked() else "unlocked"
        waiters = len(self._waiters)
        return f"<Condition [{state}, waiters:{waiters}]>"


class Semaphore:
    def __init__(self, value: int = 1) -> None:
        if value < 0:
            raise ValueError("Semaphore initial value must be >= 0")
        self._sem_handle: int = molt_asyncio_semaphore_new(value)
        self._waiters: _deque[Future] = _deque()

    def locked(self) -> bool:
        return molt_asyncio_semaphore_value(self._sem_handle) == 0

    async def acquire(self) -> bool:
        if molt_asyncio_semaphore_acquire_fast(self._sem_handle):
            return True
        fut = Future()
        self._waiters.append(fut)
        try:
            await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                _asyncio_waiters_remove(self._waiters, fut)
            raise
        return True

    def release(self) -> None:
        molt_asyncio_semaphore_release_fast(self._sem_handle, -1)
        if self._waiters:
            _asyncio_waiters_notify(self._waiters, 1, True)

    async def __aenter__(self) -> "Semaphore":
        await self.acquire()
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.release()

    def __repr__(self) -> str:
        value = int(molt_asyncio_semaphore_value(self._sem_handle))
        state = "locked" if value == 0 else f"unlocked, value:{value}"
        return f"<Semaphore [{state}]>"

    def __del__(self) -> None:
        handle = getattr(self, "_sem_handle", None)
        if handle is not None:
            molt_asyncio_semaphore_drop(handle)


class BoundedSemaphore(Semaphore):
    def __init__(self, value: int = 1) -> None:
        super().__init__(value)
        self._initial_value = value

    def release(self) -> None:
        molt_asyncio_semaphore_release_fast(self._sem_handle, self._initial_value)
        if self._waiters:
            _asyncio_waiters_notify(self._waiters, 1, True)


class Barrier:
    def __init__(self, parties: int) -> None:
        if parties <= 0:
            raise ValueError("Barrier parties must be > 0")
        self._parties = parties
        self._count = 0
        self._waiters: list[Future] = []
        self._broken = False

    async def wait(self) -> int:
        if self._broken:
            raise BrokenBarrierError("Barrier broken")
        fut = Future()
        self._waiters.append(fut)
        self._count += 1
        if self._count == self._parties:
            self._count = 0
            # Set the result of each waiter to its index.
            waiters = self._waiters
            self._waiters = []
            for i, w in enumerate(waiters):
                if not w.done():
                    w.set_result(i)
        try:
            return await fut
        except BaseException as exc:
            if _is_cancelled_exc(exc):
                _asyncio_waiters_remove(self._waiters, fut)
                if self._count > 0:
                    self._count -= 1
            raise

    @property
    def parties(self) -> int:
        return self._parties

    @property
    def n_waiting(self) -> int:
        return self._count

    @property
    def broken(self) -> bool:
        return self._broken

    async def reset(self) -> None:
        waiters = self._waiters
        self._waiters = []
        self._count = 0
        self._broken = False
        # Wake all waiters with BrokenBarrierError.
        for w in waiters:
            if not w.done():
                w.set_exception(BrokenBarrierError("Barrier was reset"))

    async def abort(self) -> None:
        self._broken = True
        waiters = self._waiters
        self._waiters = []
        self._count = 0
        # Wake all waiters with BrokenBarrierError.
        for w in waiters:
            if not w.done():
                w.set_exception(BrokenBarrierError("Barrier was aborted"))

    async def __aenter__(self) -> int:
        return await self.wait()

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        pass

    def __repr__(self) -> str:
        if self._broken:
            state = "broken"
        else:
            state = "filling"
        return f"<Barrier [{state}, waiters:{self._count}/{self._parties}]>"

__all__ = [
    "Barrier",
    "BoundedSemaphore",
    "BrokenBarrierError",
    "Condition",
    "Event",
    "Lock",
    "Semaphore",
    "collections",
    "enum",
    "exceptions",
    "mixins",
]

globals().pop("_require_intrinsic", None)
