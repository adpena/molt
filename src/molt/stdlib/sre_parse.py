"""Minimal `sre_parse` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)


def parse(_pattern: str, _flags: int = 0):
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()
    return []


__all__ = ["parse"]
