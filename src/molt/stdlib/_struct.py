"""Intrinsic-backed compatibility surface for CPython's `_struct`."""


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
