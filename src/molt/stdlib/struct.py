"""Minimal struct shim for Molt."""

from __future__ import annotations

import operator as _operator
from typing import SupportsIndex, cast

from _intrinsics import require_intrinsic as _require_intrinsic


# TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P1, status:partial): finish struct parity (buffer protocol beyond bytes/bytearray and deterministic layout policy).

__all__ = [
    "calcsize",
    "error",
    "iter_unpack",
    "pack",
    "pack_into",
    "Struct",
    "unpack_from",
    "unpack",
]


_MOLT_STRUCT_PACK = _require_intrinsic("molt_struct_pack", globals())
_MOLT_STRUCT_UNPACK = _require_intrinsic("molt_struct_unpack", globals())
_MOLT_STRUCT_CALCSIZE = _require_intrinsic("molt_struct_calcsize", globals())


class error(Exception):
    pass


class Struct:
    def __init__(self, format: object) -> None:
        normalized = _normalize_format(format)
        self.format = normalized
        self.size = calcsize(normalized)

    def pack(self, *values: object) -> bytes:
        return pack(self.format, *values)

    def unpack(self, buffer: object) -> tuple[object, ...]:
        return unpack(self.format, buffer)

    def pack_into(self, buffer: object, offset: int, *values: object) -> None:
        pack_into(self.format, buffer, offset, *values)

    def unpack_from(self, buffer: object, offset: int = 0) -> tuple[object, ...]:
        return unpack_from(self.format, buffer, offset)

    def iter_unpack(self, buffer: object):
        return iter_unpack(self.format, buffer)

    def calcsize(self) -> int:
        return calcsize(self.format)


def _index(value: object) -> int:
    return _operator.index(cast(SupportsIndex, value))


def _format_type_error(value: object) -> TypeError:
    return TypeError(
        f"Struct() argument 1 must be a str or bytes object, not {type(value).__name__}"
    )


def _normalize_format(value: object) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, bytes):
        return value.decode("latin-1")
    raise _format_type_error(value)


def _ensure_bytes_like(buffer: object, *, writable: bool) -> memoryview:
    if not isinstance(buffer, (bytes, bytearray, memoryview)):
        raise TypeError(
            f"a bytes-like object is required, not '{type(buffer).__name__}'"
        )
    view = memoryview(buffer)
    if isinstance(buffer, memoryview) and getattr(view, "c_contiguous", True) is False:
        if writable:
            raise TypeError(
                f"argument must be read-write bytes-like object, not {type(buffer).__name__}"
            )
        raise BufferError("memoryview: underlying buffer is not C-contiguous")
    if writable and view.readonly:
        raise TypeError(
            f"argument must be read-write bytes-like object, not {type(buffer).__name__}"
        )
    return view


def pack(format: object, *values: object) -> bytes:
    fmt = _normalize_format(format)
    try:
        return _MOLT_STRUCT_PACK(fmt, values)
    except (TypeError, ValueError, OverflowError) as exc:
        raise error(str(exc)) from None
    except BufferError:
        raise
    except Exception as exc:
        raise error(str(exc)) from None


def unpack(format: object, buffer: object) -> tuple[object, ...]:
    intrinsic = _require_intrinsic("molt_struct_unpack", _MOLT_STRUCT_UNPACK)
    fmt = _normalize_format(format)
    view = _ensure_bytes_like(buffer, writable=False)
    try:
        return intrinsic(fmt, view)
    except (TypeError, ValueError, OverflowError) as exc:
        raise error(str(exc)) from None
    except BufferError:
        raise
    except Exception as exc:
        raise error(str(exc)) from None


def calcsize(format: object) -> int:
    fmt = _normalize_format(format)
    try:
        return int(_MOLT_STRUCT_CALCSIZE(fmt))
    except (TypeError, ValueError, OverflowError) as exc:
        raise error(str(exc)) from None
    except Exception as exc:
        raise error(str(exc)) from None


def pack_into(format: object, buffer: object, offset: object, *values: object) -> None:
    fmt = _normalize_format(format)
    data = pack(fmt, *values)
    view = _ensure_bytes_like(buffer, writable=True)
    start = _index(offset)
    size = len(data)
    buf_len = len(view)
    if start < -buf_len:
        raise error(f"offset {start} out of range for {buf_len}-byte buffer")
    if start < 0:
        raise error(f"no space to pack {size} bytes at offset {start}")
    if start + size > buf_len:
        raise error(
            "pack_into requires a buffer of at least "
            f"{start + size} bytes for packing {size} bytes at offset {start} "
            f"(actual buffer size is {buf_len})"
        )
    view[start : start + size] = data
    return None


def unpack_from(
    format: object, buffer: object, offset: object = 0
) -> tuple[object, ...]:
    fmt = _normalize_format(format)
    view = _ensure_bytes_like(buffer, writable=False)
    start = _index(offset)
    size = calcsize(fmt)
    buf_len = len(view)
    if start < -buf_len:
        raise error(f"offset {start} out of range for {buf_len}-byte buffer")
    if start < 0:
        raise error(f"not enough data to unpack {size} bytes at offset {start}")
    if start + size > buf_len:
        raise error(
            "unpack_from requires a buffer of at least "
            f"{start + size} bytes for unpacking {size} bytes at offset {start} "
            f"(actual buffer size is {buf_len})"
        )
    data = view[start : start + size].tobytes()
    return unpack(fmt, data)


def iter_unpack(format: object, buffer: object):
    fmt = _normalize_format(format)
    size = calcsize(fmt)
    if size == 0:
        raise error("cannot iteratively unpack with a struct of length 0")
    view = _ensure_bytes_like(buffer, writable=False)
    length = len(view)
    if length % size != 0:
        raise error(
            f"iterative unpacking requires a buffer of a multiple of {size} bytes"
        )

    def _iterator():
        offset = 0
        while offset < length:
            yield unpack_from(fmt, view, offset)
            offset += size

    return _iterator()
