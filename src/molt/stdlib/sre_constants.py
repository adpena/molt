"""Minimal `sre_constants` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)
_MOLT_IMPORT_SMOKE_RUNTIME_READY()

OPCODES: tuple[str, ...] = (
    "FAILURE",
    "SUCCESS",
    "LITERAL",
    "NOT_LITERAL",
    "IN",
    "ANY",
    "AT",
)

__all__ = ["OPCODES"]
