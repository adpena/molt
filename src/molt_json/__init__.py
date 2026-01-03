"""CPython fallback for the Molt JSON package.

The real Molt package is implemented in Rust/WASM. This shim keeps tests and
local tooling working in CPython environments.
"""

from __future__ import annotations

import ctypes
import json
import re
import struct
from typing import Any

from molt import shims

_INT_RE = re.compile(r"-?(0|[1-9]\\d*)\\Z")
_QNAN = 0x7FF8_0000_0000_0000
_TAG_INT = 0x0001_0000_0000_0000
_TAG_BOOL = 0x0002_0000_0000_0000
_TAG_NONE = 0x0003_0000_0000_0000
_TAG_PTR = 0x0004_0000_0000_0000
_TAG_PENDING = 0x0005_0000_0000_0000
_TAG_MASK = 0x0007_0000_0000_0000
_POINTER_MASK = 0x0000_FFFF_FFFF_FFFF

_TYPE_STRING = 200
_TYPE_LIST = 201
_TYPE_BYTES = 202
_TYPE_DICT = 204
_TYPE_TUPLE = 206

_ITER_READY = False


class MoltHeader(ctypes.Structure):
    _fields_ = [
        ("type_id", ctypes.c_uint32),
        ("ref_count", ctypes.c_uint32),
        ("poll_fn", ctypes.c_uint64),
        ("state", ctypes.c_int64),
        ("size", ctypes.c_size_t),
    ]


class RustVec(ctypes.Structure):
    _fields_ = [
        ("data", ctypes.c_void_p),
        ("len", ctypes.c_size_t),
        ("cap", ctypes.c_size_t),
    ]


def _parse_int_runtime(data: str) -> int:
    lib = shims.load_runtime()
    if lib is None:
        raise RuntimeError("Molt runtime library not available")
    buf = data.encode("utf-8")
    return int(lib.molt_json_parse_int(buf, len(buf)))


def _is_none_bits(bits: int) -> bool:
    return (bits & (_QNAN | _TAG_MASK)) == (_QNAN | _TAG_NONE)


def _prepare_iter_runtime(lib: ctypes.CDLL) -> None:
    global _ITER_READY
    if _ITER_READY:
        return
    if hasattr(lib, "molt_iter"):
        lib.molt_iter.argtypes = [ctypes.c_uint64]
        lib.molt_iter.restype = ctypes.c_uint64
    if hasattr(lib, "molt_iter_next"):
        lib.molt_iter_next.argtypes = [ctypes.c_uint64]
        lib.molt_iter_next.restype = ctypes.c_uint64
    if hasattr(lib, "molt_dict_items"):
        lib.molt_dict_items.argtypes = [ctypes.c_uint64]
        lib.molt_dict_items.restype = ctypes.c_uint64
    if hasattr(lib, "molt_dec_ref_obj"):
        lib.molt_dec_ref_obj.argtypes = [ctypes.c_uint64]
        lib.molt_dec_ref_obj.restype = None
    _ITER_READY = True


def _decode_iterable_runtime(bits: int, kind: str) -> Any | None:
    lib = shims.load_runtime()
    if (
        lib is None
        or not hasattr(lib, "molt_iter")
        or not hasattr(lib, "molt_iter_next")
    ):
        return None
    if kind == "dict" and not hasattr(lib, "molt_dict_items"):
        return None
    _prepare_iter_runtime(lib)
    view_bits = None
    if kind == "dict":
        view_bits = lib.molt_dict_items(bits)
        if _is_none_bits(view_bits):
            return None
        iter_bits = lib.molt_iter(view_bits)
    else:
        iter_bits = lib.molt_iter(bits)
    if _is_none_bits(iter_bits):
        if view_bits is not None and hasattr(lib, "molt_dec_ref_obj"):
            lib.molt_dec_ref_obj(view_bits)
        return None

    items: list[Any] = []
    dict_out: dict[Any, Any] = {}
    try:
        while True:
            pair_bits = lib.molt_iter_next(iter_bits)
            pair = _decode_molt_object(pair_bits)
            if hasattr(lib, "molt_dec_ref_obj"):
                lib.molt_dec_ref_obj(pair_bits)
            if not isinstance(pair, tuple) or len(pair) != 2:
                return None
            value, done = pair
            if done:
                break
            if kind == "dict":
                if not isinstance(value, tuple) or len(value) != 2:
                    return None
                key, val = value
                dict_out[key] = val
            else:
                items.append(value)
    finally:
        if hasattr(lib, "molt_dec_ref_obj"):
            lib.molt_dec_ref_obj(iter_bits)
            if view_bits is not None:
                lib.molt_dec_ref_obj(view_bits)
    if kind == "tuple":
        return tuple(items)
    if kind == "dict":
        return dict_out
    return items


def _decode_molt_object(bits: int) -> Any:
    if (bits & _QNAN) != _QNAN:
        packed = bits.to_bytes(8, byteorder="little", signed=False)
        return struct.unpack("d", packed)[0]
    if (bits & (_QNAN | _TAG_MASK)) == (_QNAN | _TAG_INT):
        raw = bits & _POINTER_MASK
        sign_bit = 1 << 46
        if raw & sign_bit:
            raw = raw - (1 << 47)
        return int(raw)
    if (bits & (_QNAN | _TAG_MASK)) == (_QNAN | _TAG_BOOL):
        return bool(bits & 0x1)
    if (bits & (_QNAN | _TAG_MASK)) == (_QNAN | _TAG_NONE):
        return None
    if (bits & (_QNAN | _TAG_MASK)) == (_QNAN | _TAG_PENDING):
        raise RuntimeError("molt_json parse returned pending")
    if (bits & (_QNAN | _TAG_MASK)) == (_QNAN | _TAG_PTR):
        ptr = bits & _POINTER_MASK
        header_ptr = ptr - ctypes.sizeof(MoltHeader)
        header = MoltHeader.from_address(header_ptr)
        if header.type_id == _TYPE_STRING:
            length = ctypes.c_size_t.from_address(ptr).value
            data_ptr = ptr + ctypes.sizeof(ctypes.c_size_t)
            data = ctypes.string_at(data_ptr, length)
            return data.decode("utf-8")
        if header.type_id == _TYPE_BYTES:
            length = ctypes.c_size_t.from_address(ptr).value
            data_ptr = ptr + ctypes.sizeof(ctypes.c_size_t)
            return ctypes.string_at(data_ptr, length)
        if header.type_id == _TYPE_LIST:
            decoded = _decode_iterable_runtime(bits, "list")
            if decoded is not None:
                return decoded
            vec_ptr = ctypes.c_void_p.from_address(ptr).value
            if not vec_ptr:
                return []
            vec = RustVec.from_address(vec_ptr)
            list_out: list[Any] = []
            for idx in range(vec.len):
                elem_bits = ctypes.c_uint64.from_address(
                    vec.data + idx * ctypes.sizeof(ctypes.c_uint64)
                ).value
                list_out.append(_decode_molt_object(elem_bits))
            return list_out
        if header.type_id == _TYPE_DICT:
            decoded = _decode_iterable_runtime(bits, "dict")
            if decoded is not None:
                return decoded
            order_ptr = ctypes.c_void_p.from_address(ptr).value
            if not order_ptr:
                return {}
            vec = RustVec.from_address(order_ptr)
            dict_out: dict[Any, Any] = {}
            for idx in range(0, vec.len, 2):
                key_bits = ctypes.c_uint64.from_address(
                    vec.data + idx * ctypes.sizeof(ctypes.c_uint64)
                ).value
                val_bits = ctypes.c_uint64.from_address(
                    vec.data + (idx + 1) * ctypes.sizeof(ctypes.c_uint64)
                ).value
                dict_out[_decode_molt_object(key_bits)] = _decode_molt_object(val_bits)
            return dict_out
        if header.type_id == _TYPE_TUPLE:
            decoded = _decode_iterable_runtime(bits, "tuple")
            if decoded is not None:
                return decoded
            vec_ptr = ctypes.c_void_p.from_address(ptr).value
            if not vec_ptr:
                return ()
            vec = RustVec.from_address(vec_ptr)
            items: list[Any] = []
            for idx in range(vec.len):
                elem_bits = ctypes.c_uint64.from_address(
                    vec.data + idx * ctypes.sizeof(ctypes.c_uint64)
                ).value
                items.append(_decode_molt_object(elem_bits))
            return tuple(items)
        raise RuntimeError(f"Unsupported MoltObject type_id {header.type_id}")
    raise RuntimeError("Unsupported MoltObject encoding")


def _parse_scalar_runtime(data: str) -> Any:
    lib = shims.load_runtime()
    if lib is None or not hasattr(lib, "molt_json_parse_scalar"):
        raise RuntimeError("Molt runtime scalar parser not available")
    buf = data.encode("utf-8")
    out_ptr_c = ctypes.c_uint64()
    rc = lib.molt_json_parse_scalar(buf, len(buf), ctypes.byref(out_ptr_c))
    if rc != 0:
        raise RuntimeError("molt_json scalar parse failed")
    return _decode_molt_object(out_ptr_c.value)


def parse(data: str) -> Any:
    trimmed = data.strip()
    lib = shims.load_runtime()
    if lib is not None:
        try:
            return _parse_scalar_runtime(trimmed)
        except Exception:
            if _INT_RE.fullmatch(trimmed):
                return _parse_int_runtime(trimmed)
    return json.loads(data)
