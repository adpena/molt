"""Minimal `dis` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)


def dis(_obj=None, *, file=None) -> None:
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()
    _ = file


__all__ = ["dis"]
