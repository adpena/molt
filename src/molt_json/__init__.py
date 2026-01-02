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
_TAG_PENDING = 0x0004_0000_0000_0000
_POINTER_MASK = 0x0000_FFFF_FFFF_FFFF


def _parse_int_runtime(data: str) -> int:
    lib = shims.load_runtime()
    if lib is None:
        raise RuntimeError("Molt runtime library not available")
    buf = data.encode("utf-8")
    return int(lib.molt_json_parse_int(buf, len(buf)))


def _decode_molt_object(bits: int) -> Any:
    if (bits & _QNAN) != _QNAN:
        packed = bits.to_bytes(8, byteorder="little", signed=False)
        return struct.unpack("d", packed)[0]
    if (bits & (_QNAN | _TAG_INT)) == (_QNAN | _TAG_INT):
        raw = bits & _POINTER_MASK
        sign_bit = 1 << 46
        if raw & sign_bit:
            raw = raw - (1 << 47)
        return int(raw)
    if (bits & (_QNAN | _TAG_BOOL)) == (_QNAN | _TAG_BOOL):
        return bool(bits & 0x1)
    if (bits & (_QNAN | _TAG_NONE)) == (_QNAN | _TAG_NONE):
        return None
    if (bits & (_QNAN | _TAG_PENDING)) == (_QNAN | _TAG_PENDING):
        raise RuntimeError("molt_json parse returned pending")
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
