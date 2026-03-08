"""Molt-native concurrency helpers outside the CPython stdlib namespace."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from molt import intrinsics as _intrinsics

if TYPE_CHECKING:
    from moltlib import _concurrency_runtime as _concurrency

    CancellationToken = _concurrency.CancellationToken
    Channel = _concurrency.Channel
    cancel_current = _concurrency.cancel_current
    cancelled = _concurrency.cancelled
    channel = _concurrency.channel
    current_token = _concurrency.current_token
    set_current_token = _concurrency.set_current_token
    spawn = _concurrency.spawn

__all__ = [
    "CancellationToken",
    "Channel",
    "cancel_current",
    "cancelled",
    "channel",
    "current_token",
    "set_current_token",
    "spawn",
]
_RUNTIME_EXPORTS = set(__all__) | {"_call_intrinsic"}


def __getattr__(name: str) -> Any:
    if name not in _RUNTIME_EXPORTS:
        raise AttributeError(name)
    if not _intrinsics.runtime_active():
        raise RuntimeError(
            "molt runtime intrinsics are unavailable outside compiled binaries"
        )
    from moltlib import _concurrency_runtime as _concurrency

    value = getattr(_concurrency, name)
    globals()[name] = value
    return value


def __dir__() -> list[str]:
    return sorted(set(__all__) | set(globals()))
