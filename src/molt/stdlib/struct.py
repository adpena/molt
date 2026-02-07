"""Minimal struct shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


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
_MOLT_STRUCT_PACK_INTO = _require_intrinsic("molt_struct_pack_into", globals())
_MOLT_STRUCT_UNPACK_FROM = _require_intrinsic("molt_struct_unpack_from", globals())
_MOLT_STRUCT_ITER_UNPACK = _require_intrinsic("molt_struct_iter_unpack", globals())


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


def pack(format: object, *values: object) -> bytes:
    fmt = _normalize_format(format)
    try:
        return _MOLT_STRUCT_PACK(fmt, values)
    except (TypeError, ValueError, OverflowError) as exc:
        raise error(str(exc)) from None


def unpack(format: object, buffer: object) -> tuple[object, ...]:
    fmt = _normalize_format(format)
    try:
        return _MOLT_STRUCT_UNPACK(fmt, buffer)
    except (ValueError, OverflowError) as exc:
        raise error(str(exc)) from None
    except BufferError:
        raise


def calcsize(format: object) -> int:
    fmt = _normalize_format(format)
    try:
        return int(_MOLT_STRUCT_CALCSIZE(fmt))
    except (TypeError, ValueError, OverflowError) as exc:
        raise error(str(exc)) from None


def pack_into(format: object, buffer: object, offset: object, *values: object) -> None:
    data = pack(format, *values)
    try:
        _MOLT_STRUCT_PACK_INTO(buffer, offset, data)
    except (ValueError, OverflowError) as exc:
        raise error(str(exc)) from None


def unpack_from(
    format: object, buffer: object, offset: object = 0
) -> tuple[object, ...]:
    fmt = _normalize_format(format)
    try:
        return _MOLT_STRUCT_UNPACK_FROM(fmt, buffer, offset)
    except (ValueError, OverflowError) as exc:
        raise error(str(exc)) from None
    except BufferError:
        raise


def iter_unpack(format: object, buffer: object):
    fmt = _normalize_format(format)
    try:
        unpacked = _MOLT_STRUCT_ITER_UNPACK(fmt, buffer)
    except (ValueError, OverflowError) as exc:
        raise error(str(exc)) from None
    except BufferError:
        raise
    return iter(unpacked)
