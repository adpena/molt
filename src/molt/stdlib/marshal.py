"""Minimal `marshal` subset for Molt."""

from __future__ import annotations

import json

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)


def dumps(value, version: int = 4) -> bytes:
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()
    _ = version
    return json.dumps(value, sort_keys=True).encode("utf-8")


def loads(data: bytes):
    _MOLT_IMPORT_SMOKE_RUNTIME_READY()
    return json.loads(bytes(data).decode("utf-8"))


__all__ = ["dumps", "loads"]
