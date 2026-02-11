"""Minimal `tracemalloc` subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)


_TRACING = False


def start(_nframe: int = 1) -> None:
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()
    global _TRACING
    _TRACING = True


def stop() -> None:
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()
    global _TRACING
    _TRACING = False


def is_tracing() -> bool:
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()
    return _TRACING


def get_traced_memory() -> tuple[int, int]:
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()
    return (0, 0)


__all__ = ["start", "stop", "is_tracing", "get_traced_memory"]
