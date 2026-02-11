"""Minimal `cProfile` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from profile import Profile

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)
_MOLT_IMPORT_SMOKE_RUNTIME_READY()

__all__ = ["Profile"]
