"""Minimal `sre_parse` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")


def parse(
    _pattern: str,
    _flags: int = 0,
    _runtime_ready_intrinsic=_MOLT_IMPORT_SMOKE_RUNTIME_READY,
):
    _runtime_ready_intrinsic()
    return []


__all__ = ["parse"]

del _MOLT_IMPORT_SMOKE_RUNTIME_READY

globals().pop("_require_intrinsic", None)
