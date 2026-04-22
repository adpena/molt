"""Intrinsic-backed compatibility surface for CPython's `_struct`."""

from _intrinsics import require_intrinsic as _require_intrinsic

from struct import (
    Struct,
    calcsize,
    error,
    iter_unpack,
    pack,
    pack_into,
    unpack,
    unpack_from,
)

_require_intrinsic("molt_struct_pack")

__all__ = [
    "Struct",
    "calcsize",
    "error",
    "iter_unpack",
    "pack",
    "pack_into",
    "unpack",
    "unpack_from",
]

globals().pop("_require_intrinsic", None)
