"""Molt core package.

Layering:
- `molt`: compiler, runtime, and tooling core
- `builtins` / `molt.stdlib.*`: CPython-compatible builtins + stdlib surfaces
- `moltlib.*`: Molt-specific user-facing libraries

Compatibility submodules for moved Molt-only helpers remain under `molt.*`,
but the root package does not re-export them.
"""

from __future__ import annotations

_MOVED_TO_MOLTLIB = {
    "CancellationToken": ("moltlib.concurrency", "molt.concurrency"),
    "Channel": ("moltlib.concurrency", "molt.concurrency"),
    "cancel_current": ("moltlib.concurrency", "molt.concurrency"),
    "cancelled": ("moltlib.concurrency", "molt.concurrency"),
    "channel": ("moltlib.concurrency", "molt.concurrency"),
    "current_token": ("moltlib.concurrency", "molt.concurrency"),
    "set_current_token": ("moltlib.concurrency", "molt.concurrency"),
    "spawn": ("moltlib.concurrency", "molt.concurrency"),
    "Request": ("moltlib.net", "molt.net"),
    "Response": ("moltlib.net", "molt.net"),
    "Stream": ("moltlib.net", "molt.net"),
    "StreamSender": ("moltlib.net", "molt.net"),
    "WebSocket": ("moltlib.net", "molt.net"),
    "stream": ("moltlib.net", "molt.net"),
    "stream_channel": ("moltlib.net", "molt.net"),
    "ws_connect": ("moltlib.net", "molt.net"),
    "ws_pair": ("moltlib.net", "molt.net"),
}

__all__: list[str] = []


def __getattr__(name: str) -> object:
    moved = _MOVED_TO_MOLTLIB.get(name)
    if moved is not None:
        canonical_module, compat_module = moved
        raise AttributeError(
            f"module 'molt' has no attribute {name!r}; use "
            f"{canonical_module}.{name} (compatibility shim: {compat_module}.{name})"
        )
    raise AttributeError(name)


def __dir__() -> list[str]:
    return sorted(set(globals()))
