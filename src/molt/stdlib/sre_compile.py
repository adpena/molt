"""Minimal `sre_compile` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)


def compile(_pattern, _flags: int = 0):
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()
    return None


__all__ = ["compile"]
