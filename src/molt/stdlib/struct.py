"""Minimal struct shim for Molt."""

from __future__ import annotations

from typing import Any

import builtins as _builtins

# TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): implement full struct parity (alignment, full format table, pack_into/unpack_from/iter_unpack).

__all__ = [
    "calcsize",
    "error",
    "pack",
    "Struct",
    "unpack",
]


def _load_intrinsic(name: str) -> Any | None:
    direct = globals().get(name)
    if direct is not None:
        return direct
    return getattr(_builtins, name, None)


_MOLT_STRUCT_PACK = _load_intrinsic("_molt_struct_pack")
_MOLT_STRUCT_UNPACK = _load_intrinsic("_molt_struct_unpack")
_MOLT_STRUCT_CALCSIZE = _load_intrinsic("_molt_struct_calcsize")


class error(Exception):
    pass


class Struct:
    def __init__(self, format: str) -> None:
        self.format = format

    def pack(self, *values: object) -> bytes:
        return pack(self.format, *values)

    def unpack(self, buffer: object) -> tuple[object, ...]:
        return unpack(self.format, buffer)

    def calcsize(self) -> int:
        return calcsize(self.format)


def _require_intrinsic(name: str, intrinsic: Any | None) -> Any:
    if intrinsic is None:
        raise ImportError(f"{name} intrinsic unavailable")
    return intrinsic


def pack(format: str, *values: object) -> bytes:
    intrinsic = _require_intrinsic("_molt_struct_pack", _MOLT_STRUCT_PACK)
    try:
        return intrinsic(format, values)
    except (TypeError, ValueError, OverflowError) as exc:
        raise error(str(exc)) from None


def unpack(format: str, buffer: object) -> tuple[object, ...]:
    intrinsic = _require_intrinsic("_molt_struct_unpack", _MOLT_STRUCT_UNPACK)
    try:
        return intrinsic(format, buffer)
    except (TypeError, ValueError, OverflowError) as exc:
        raise error(str(exc)) from None


def calcsize(format: str) -> int:
    intrinsic = _require_intrinsic("_molt_struct_calcsize", _MOLT_STRUCT_CALCSIZE)
    try:
        return int(intrinsic(format))
    except (TypeError, ValueError, OverflowError) as exc:
        raise error(str(exc)) from None
