"""Public API surface shim for ``asyncio.exceptions``."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

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

import sys as _aex_cleanup_sys

_aex_cleanup_dict = (
    getattr(_aex_cleanup_sys.modules.get(__name__), "__dict__", None) or globals()
)
for _name in ("QueueEmpty", "QueueFull", "QueueShutDown"):
    _aex_cleanup_dict.pop(_name, None)
del _aex_cleanup_sys, _aex_cleanup_dict
