"""Minimal `faulthandler` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)


def enable(*_args, **_kwargs) -> None:
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()


def disable() -> None:
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()


def is_enabled() -> bool:
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()
    return False


__all__ = ["enable", "disable", "is_enabled"]
