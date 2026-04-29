"""Molt: Python → Native/WASM compiler research project."""

from __future__ import annotations

from ._version import version as _resolve_version

__version__ = _resolve_version()

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


def __getattr__(name: str):
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
