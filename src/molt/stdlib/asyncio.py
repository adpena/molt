"""Capability-gated asyncio shim for Molt."""

from __future__ import annotations

import builtins
from typing import Any

__all__ = ["run", "sleep"]


def _missing(*_args: Any, **_kwargs: Any) -> Any:
    raise RuntimeError("molt shims not installed")


molt_block_on = getattr(builtins, "molt_block_on", _missing)
molt_async_sleep = getattr(builtins, "molt_async_sleep", _missing)


def run(awaitable: Any) -> Any:
    return molt_block_on(awaitable)


def sleep(_delay: float = 0.0) -> Any:
    return molt_async_sleep()
