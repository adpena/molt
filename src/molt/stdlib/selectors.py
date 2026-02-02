"""Selectors shim for Molt."""

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement full selectors API (SelectorKey namedtuple parity, selector-specific implementations, and unregister semantics across platforms).

from __future__ import annotations

from typing import Any, NamedTuple
import time as _time
import builtins as _builtins
import sys as _sys

import asyncio as _asyncio

EVENT_READ = 1
EVENT_WRITE = 2


class SelectorKey(NamedTuple):
    fileobj: Any
    fd: int
    events: int
    data: Any


def _fileobj_to_fd(fileobj: Any) -> int:
    if isinstance(fileobj, int):
        return fileobj
    if hasattr(fileobj, "fileno"):
        return int(fileobj.fileno())
    raise ValueError("fileobj must be a file descriptor or have fileno()")


def _fileobj_to_handle(fileobj: Any) -> Any:
    if isinstance(fileobj, int):
        return fileobj
    if hasattr(fileobj, "_handle"):
        return getattr(fileobj, "_handle")
    raise ValueError("fileobj must be a socket or file descriptor")


def _load_intrinsic(name: str) -> Any | None:
    direct = globals().get(name)
    if direct is not None:
        return direct
    return getattr(_builtins, name, None)


# Force builtin_func emission for intrinsics referenced via string lookups.
try:
    _molt_io_wait_new = _molt_io_wait_new  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _molt_io_wait_new = None
try:
    molt_block_on = molt_block_on  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    molt_block_on = None

_molt_io_wait_new = _load_intrinsic("_molt_io_wait_new")
_molt_block_on = _load_intrinsic("molt_block_on")


class BaseSelector:
    def __init__(self) -> None:
        self._map: dict[int, SelectorKey] = {}

    def register(self, fileobj: Any, events: int, data: Any = None) -> SelectorKey:
        if events & (EVENT_READ | EVENT_WRITE) == 0:
            raise ValueError("events must be EVENT_READ or EVENT_WRITE")
        if _molt_io_wait_new is not None:
            _fileobj_to_handle(fileobj)
        fd = _fileobj_to_fd(fileobj)
        if fd in self._map:
            raise KeyError("fileobj is already registered")
        key = SelectorKey(fileobj, fd, events, data)
        self._map[fd] = key
        return key

    def unregister(self, fileobj: Any) -> SelectorKey:
        fd = _fileobj_to_fd(fileobj)
        if fd not in self._map:
            raise KeyError("fileobj is not registered")
        return self._map.pop(fd)

    def modify(self, fileobj: Any, events: int, data: Any = None) -> SelectorKey:
        fd = _fileobj_to_fd(fileobj)
        if fd not in self._map:
            raise KeyError("fileobj is not registered")
        key = SelectorKey(fileobj, fd, events, data)
        self._map[fd] = key
        return key

    def get_key(self, fileobj: Any) -> SelectorKey:
        fd = _fileobj_to_fd(fileobj)
        if fd not in self._map:
            raise KeyError("fileobj is not registered")
        return self._map[fd]

    def get_map(self) -> dict[int, SelectorKey]:
        return dict(self._map)

    def close(self) -> None:
        self._map.clear()

    def select(self, timeout: float | None = None) -> list[tuple[SelectorKey, int]]:
        if not self._map:
            if timeout is None:
                return []
            if timeout > 0:
                _time.sleep(timeout)
            return []
        io_wait = _molt_io_wait_new
        block_on = _molt_block_on
        if io_wait is None or block_on is None:
            raise RuntimeError("selector intrinsics not available")

        # TODO(perf, owner:stdlib, milestone:SL2, priority:P2, status:planned): implement a shared poller path to avoid allocating per-fd futures on each select().
        async def _wait_ready() -> list[tuple[SelectorKey, int]]:
            ensure_future = getattr(_asyncio, "ensure_future", None)
            futures: list[tuple[SelectorKey, Any]] = []
            for key in self._map.values():
                handle = _fileobj_to_handle(key.fileobj)
                fut = io_wait(handle, key.events, None)
                if ensure_future is not None:
                    fut = ensure_future(fut)
                futures.append((key, fut))
            done, pending = await _asyncio.wait(
                [f for _, f in futures],
                timeout=timeout,
                return_when=_asyncio.FIRST_COMPLETED,
            )
            ready: list[tuple[SelectorKey, int]] = []
            for key, fut in futures:
                if fut in done:
                    try:
                        mask = int(fut.result())
                    except TimeoutError:
                        mask = 0
                    if mask:
                        ready.append((key, mask))
            for fut in pending:
                try:
                    fut.cancel()
                except Exception:
                    pass
            return ready

        return block_on(_wait_ready())


class DefaultSelector(BaseSelector):
    pass


SelectSelector = DefaultSelector
PollSelector = DefaultSelector
_IS_LINUX = _sys.platform.startswith("linux")
_HAS_KQUEUE = not _IS_LINUX and _sys.platform != "win32"

if _IS_LINUX:
    EpollSelector = DefaultSelector

if _HAS_KQUEUE:
    KqueueSelector = DefaultSelector
