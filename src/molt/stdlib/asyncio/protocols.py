"""Public API surface shim for ``asyncio.protocols``."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from asyncio import (
    BaseProtocol,
    BufferedProtocol,
    DatagramProtocol,
    Protocol,
    SubprocessProtocol,
)

__all__ = [
    "BaseProtocol",
    "BufferedProtocol",
    "DatagramProtocol",
    "Protocol",
    "SubprocessProtocol",
]

globals().pop("_require_intrinsic", None)
