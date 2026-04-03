"""Molt: Python → Native/WASM compiler research project."""

from __future__ import annotations

__version__ = "0.1.0-alpha"

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

# Runtime symbols are NOT part of the core namespace; they live in
# moltlib.concurrency / moltlib.net and are only available when the
# Molt runtime is active inside a compiled binary.
__all__: list[str] = []


def __getattr__(name: str) -> Any:
    if name in _CONCURRENCY_EXPORTS:
        raise AttributeError(
            f"moltlib.concurrency.{name} — use 'from moltlib.concurrency import {name}'"
        )
    if name in _NET_EXPORTS:
        raise AttributeError(
            f"moltlib.net.{name} — use 'from moltlib.net import {name}'"
        )
    raise AttributeError(name)


def __dir__() -> list[str]:
    return sorted(set(__all__) | set(globals()))
