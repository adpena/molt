"""Intrinsic-backed compatibility surface for CPython's `_ctypes`."""

from _intrinsics import require_intrinsic as _require_intrinsic

from ctypes import (
    Array,
    Structure,
    _SimpleCData,
    c_bool,
    c_byte,
    c_char,
    c_double,
    c_float,
    c_int,
    c_int8,
    c_int16,
    c_int32,
    c_int64,
    c_long,
    c_longlong,
    c_short,
    c_size_t,
    c_ubyte,
    c_uint,
    c_uint8,
    c_uint16,
    c_uint32,
    c_uint64,
    c_ulong,
    c_ulonglong,
    c_ushort,
    c_void_p,
    pointer,
    sizeof,
)

_MOLT_CTYPES_REQUIRE_FFI = _require_intrinsic("molt_ctypes_require_ffi")

__all__ = [
    "Array",
    "Structure",
    "_SimpleCData",
    "c_bool",
    "c_byte",
    "c_char",
    "c_double",
    "c_float",
    "c_int",
    "c_int8",
    "c_int16",
    "c_int32",
    "c_int64",
    "c_long",
    "c_longlong",
    "c_short",
    "c_size_t",
    "c_ubyte",
    "c_uint",
    "c_uint8",
    "c_uint16",
    "c_uint32",
    "c_uint64",
    "c_ulong",
    "c_ulonglong",
    "c_ushort",
    "c_void_p",
    "pointer",
    "sizeof",
]


globals().pop("_require_intrinsic", None)
