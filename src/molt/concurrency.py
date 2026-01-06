"""Stdlib concurrency helpers for Molt channels and tasks."""

from __future__ import annotations

import asyncio
import ctypes
import queue
from typing import Any, Callable, Coroutine, Generic, SupportsInt, TypeVar, cast

from molt import shims

T = TypeVar("T")


class Channel(Generic[T]):
    def __init__(self, handle: Any, maxsize: int = 0) -> None:
        self._handle = handle
        self._maxsize = maxsize

    def send(self, value: T) -> int:
        return shims.molt_chan_send(self._handle, value)

    def recv(self) -> T:
        return cast(T, shims.molt_chan_recv(self._handle))

    async def send_async(self, value: T) -> None:
        lib = shims.load_runtime()
        runtime_handle = self._runtime_handle()
        if lib is not None and runtime_handle is not None:
            pending = shims._PENDING
            value_int = int(cast(SupportsInt, value))
            while True:
                res = int(lib.molt_chan_send(runtime_handle, value_int))
                if res != pending:
                    return
                await asyncio.sleep(0)
        if isinstance(self._handle, queue.Queue):
            await asyncio.get_running_loop().run_in_executor(
                None, self._handle.put, value
            )
            return
        await asyncio.get_running_loop().run_in_executor(
            None, shims.molt_chan_send, self._handle, value
        )

    async def recv_async(self) -> T:
        lib = shims.load_runtime()
        runtime_handle = self._runtime_handle()
        if lib is not None and runtime_handle is not None:
            pending = shims._PENDING
            while True:
                res = int(lib.molt_chan_recv(runtime_handle))
                if res != pending:
                    return cast(T, res)
                await asyncio.sleep(0)
        if isinstance(self._handle, queue.Queue):
            result = await asyncio.get_running_loop().run_in_executor(
                None, self._handle.get
            )
            return cast(T, result)
        result = await asyncio.get_running_loop().run_in_executor(
            None, shims.molt_chan_recv, self._handle
        )
        return cast(T, result)

    def _runtime_handle(self) -> ctypes.c_void_p | None:
        if isinstance(self._handle, ctypes.c_void_p):
            return self._handle
        if isinstance(self._handle, int):
            return ctypes.c_void_p(self._handle)
        return None


def channel(maxsize: int = 0) -> Channel[Any]:
    lib = shims.load_runtime()
    if lib is None:
        return Channel(queue.Queue(maxsize=maxsize), maxsize=maxsize)
    handle = shims.molt_chan_new(maxsize)
    return Channel(handle, maxsize=maxsize)


TaskLike = Coroutine[Any, Any, Any] | Callable[[], Coroutine[Any, Any, Any]] | Any


def spawn(task: TaskLike) -> None:
    shims.molt_spawn(task)
