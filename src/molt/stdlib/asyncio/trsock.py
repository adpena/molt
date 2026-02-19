"""Public API surface shim for ``asyncio.trsock``."""

from __future__ import annotations

import socket

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


class TransportSocket:
    pass


__all__ = ["TransportSocket", "socket"]
