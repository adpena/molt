"""CPython fallback for Molt MsgPack/CBOR parsing.

Uses runtime scalar parsers when available; otherwise falls back to Python
libraries for parity testing.
"""

from __future__ import annotations

import ctypes
from typing import Any

from molt import shims
from molt_json import _decode_molt_object


def _parse_runtime(data: bytes, fn_name: str) -> Any:
    lib = shims.load_runtime()
    if lib is None or not hasattr(lib, fn_name):
        raise RuntimeError("Molt runtime parser not available")
    out = ctypes.c_uint64()
    rc = getattr(lib, fn_name)(data, len(data), ctypes.byref(out))
    if rc != 0:
        raise RuntimeError("Molt runtime parser failed")
    return _decode_molt_object(out.value)


def parse_msgpack(data: bytes) -> Any:
    try:
        return _parse_runtime(data, "molt_msgpack_parse_scalar")
    except Exception:
        try:
            import msgpack
        except ModuleNotFoundError as exc:
            raise RuntimeError(
                "msgpack is required for parse_msgpack fallback"
            ) from exc
        return msgpack.unpackb(data, raw=False)


def parse_cbor(data: bytes) -> Any:
    try:
        return _parse_runtime(data, "molt_cbor_parse_scalar")
    except Exception:
        try:
            import cbor2
        except ModuleNotFoundError as exc:
            raise RuntimeError("cbor2 is required for parse_cbor fallback") from exc
        return cbor2.loads(data)


def parse(data: bytes) -> Any:
    return parse_msgpack(data)
