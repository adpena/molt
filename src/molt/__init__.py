"""Molt: Python → Native/WASM compiler research project."""

from __future__ import annotations

from typing import Any

from molt import intrinsics as _intrinsics

_CONCURRENCY_EXPORTS = {
    "Channel",
    "CancellationToken",
    "cancel_current",
    "cancelled",
    "channel",
    "current_token",
    "set_current_token",
    "spawn",
}
_NET_EXPORTS = {
    "Request",
    "Response",
    "Stream",
    "StreamSender",
    "WebSocket",
    "stream",
    "stream_channel",
    "ws_pair",
    "ws_connect",
}

__all__ = sorted(_CONCURRENCY_EXPORTS | _NET_EXPORTS)


def _load_runtime_symbol(name: str) -> Any:
    if name in _CONCURRENCY_EXPORTS:
        from molt import concurrency as _concurrency

        return getattr(_concurrency, name)
    if name in _NET_EXPORTS:
        from molt import net as _net

        return getattr(_net, name)
    raise AttributeError(name)


def __getattr__(name: str) -> Any:
    if name in _CONCURRENCY_EXPORTS or name in _NET_EXPORTS:
        if not _intrinsics.runtime_active():
            raise RuntimeError(
                "molt runtime intrinsics are unavailable outside compiled binaries"
            )
        value = _load_runtime_symbol(name)
        globals()[name] = value
        return value
    raise AttributeError(name)


def __dir__() -> list[str]:
    return sorted(set(__all__) | set(globals()))
