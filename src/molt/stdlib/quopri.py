"""Minimal `quopri` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)


def encodestring(data: bytes, quotetabs: bool = False, header: bool = False) -> bytes:
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()
    _ = quotetabs, header
    return bytes(data)


def decodestring(data: bytes, header: bool = False) -> bytes:
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()
    _ = header
    return bytes(data)


__all__ = ["encodestring", "decodestring"]
