"""Public API surface shim for ``asyncio.trsock``."""

from __future__ import annotations

import socket

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")


class TransportSocket:
    pass


__all__ = ["TransportSocket", "socket"]
