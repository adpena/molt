"""Exception authority for ``asyncio.exceptions``."""

from __future__ import annotations

import builtins as _builtins

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

_builtin_cancelled = getattr(_builtins, "CancelledError", None)
if _builtin_cancelled is None:

    class CancelledError(BaseException):
        pass

else:
    CancelledError = _builtin_cancelled


TimeoutError = _builtins.TimeoutError


class InvalidStateError(Exception):
    pass


class BrokenBarrierError(RuntimeError):
    pass


class LimitOverrunError(Exception):
    def __init__(self, message: str, consumed: int) -> None:
        super().__init__(message)
        self.consumed = consumed


class SendfileNotAvailableError(RuntimeError):
    pass


class IncompleteReadError(EOFError):
    def __init__(self, partial: bytes, expected: int) -> None:
        super().__init__(f"{expected} bytes expected, {len(partial)} bytes read")
        self.partial = partial
        self.expected = expected


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

globals().pop("_require_intrinsic", None)