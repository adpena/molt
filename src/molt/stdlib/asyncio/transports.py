"""Public API surface shim for ``asyncio.transports``."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from asyncio import DatagramTransport, SubprocessTransport, Transport

BaseTransport = Transport
ReadTransport = Transport
WriteTransport = Transport

__all__ = [
    "BaseTransport",
    "DatagramTransport",
    "ReadTransport",
    "SubprocessTransport",
    "Transport",
    "WriteTransport",
]

globals().pop("_require_intrinsic", None)
