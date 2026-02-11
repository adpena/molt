"""Minimal `pstats` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)
_MOLT_IMPORT_SMOKE_RUNTIME_READY()


class Stats:
    def __init__(self, *_args, **_kwargs) -> None:
        pass

    def sort_stats(self, *_args, **_kwargs) -> "Stats":
        return self

    def print_stats(self, *_args, **_kwargs) -> "Stats":
        return self


__all__ = ["Stats"]
