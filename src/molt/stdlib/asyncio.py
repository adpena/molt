"""Capability-gated asyncio shim for Molt."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

__all__ = ["run", "sleep"]

if TYPE_CHECKING:

    def molt_async_sleep() -> Any: ...

    def molt_block_on(awaitable: Any) -> Any: ...


def run(awaitable: Any) -> Any:
    return molt_block_on(awaitable)


def sleep(_delay: float = 0.0) -> Any:
    return molt_async_sleep()
