"""Molt: Python → Native/WASM compiler research project."""

from __future__ import annotations

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
    if name == "__version__":
        # The generated version contains no host metadata or filesystem probes.
        # Even lazy function imports belong to a standalone guest's closure.
        from molt._version import version

        return version()
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
    return sorted(set(__all__) | set(globals()) | {"__version__"})
