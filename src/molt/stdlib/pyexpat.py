"""Minimal `pyexpat` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)


class _Parser:
    def Parse(self, _data: bytes | str, _isfinal: bool = False) -> int:
        return 1


def ParserCreate(*_args, **_kwargs) -> _Parser:
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()
    return _Parser()


__all__ = ["ParserCreate"]
