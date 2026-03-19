"""Public API surface shim for ``asyncio.exceptions``."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from asyncio import (
    BrokenBarrierError,
    CancelledError,
    IncompleteReadError,
    InvalidStateError,
    LimitOverrunError,
    SendfileNotAvailableError,
    TimeoutError,
)

__all__ = [
    "BrokenBarrierError",
    "CancelledError",
    "IncompleteReadError",
    "InvalidStateError",
    "LimitOverrunError",
    "SendfileNotAvailableError",
    "TimeoutError",
]

for _name in ("QueueEmpty", "QueueFull", "QueueShutDown"):
    globals().pop(_name, None)
