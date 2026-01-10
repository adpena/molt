"""Capability-gated asyncio shim for Molt."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from molt.concurrency import channel

__all__ = ["Queue", "new_event_loop", "run", "set_event_loop", "sleep"]

if TYPE_CHECKING:

    def molt_async_sleep(_delay: float = 0.0, _result: Any | None = None) -> Any:
        pass

    def molt_block_on(awaitable: Any) -> Any:
        pass


def run(awaitable: Any) -> Any:
    return molt_block_on(awaitable)


def sleep(delay: float = 0.0, result: Any | None = None) -> Any:
    if result is None:
        return molt_async_sleep(delay)
    return molt_async_sleep(delay, result)


def set_event_loop(_loop: Any) -> None:
    return None


def new_event_loop() -> Any:
    return None


class Queue:
    def __init__(self, maxsize: int = 0) -> None:
        self._chan = channel(maxsize)

    async def put(self, item: Any) -> None:
        await self._chan.send_async(item)

    async def get(self) -> Any:
        return await self._chan.recv_async()
