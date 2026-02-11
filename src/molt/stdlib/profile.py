"""Minimal `profile` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)


class Profile:
    def runctx(self, code: str, globals_dict: dict, locals_dict: dict) -> None:
        _MOLT_IMPORT_SMOKE_RUNTIME_READY()
        exec(code, globals_dict, locals_dict)


__all__ = ["Profile"]
