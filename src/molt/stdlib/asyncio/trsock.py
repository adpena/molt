"""Public API surface shim for ``asyncio.trsock``."""

from __future__ import annotations

from . import _SOCKET as socket


class TransportSocket:
    pass


__all__ = ["TransportSocket", "socket"]
