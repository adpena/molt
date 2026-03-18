"""Intrinsic-backed compatibility surface for CPython's `_ctypes`."""

from _intrinsics import require_intrinsic as _require_intrinsic

from ctypes import Structure, c_int, pointer, sizeof

_MOLT_CTYPES_REQUIRE_FFI = _require_intrinsic("molt_ctypes_require_ffi")

__all__ = [
    "Structure",
    "c_int",
    "pointer",
    "sizeof",
]
