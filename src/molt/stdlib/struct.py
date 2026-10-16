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


_MOLT_STRUCT_PACK = _require_intrinsic("molt_struct_pack")
_MOLT_STRUCT_UNPACK = _require_intrinsic("molt_struct_unpack")
_MOLT_STRUCT_CALCSIZE = _require_intrinsic("molt_struct_calcsize")
_MOLT_STRUCT_PACK_INTO = _require_intrinsic("molt_struct_pack_into")
_MOLT_STRUCT_UNPACK_FROM = _require_intrinsic("molt_struct_unpack_from")
_MOLT_STRUCT_ITER_UNPACK = _require_intrinsic("molt_struct_iter_unpack")
_MOLT_STRUCT_ITERATOR_TYPE = _require_intrinsic("molt_struct_unpack_iterator_type")


class error(Exception):
    pass


# Keep the actual exception class and iterator declaration owned by this module.
_STRUCT_ERROR = error
_UNPACK_ITERATOR_TYPE = _MOLT_STRUCT_ITERATOR_TYPE()


class Struct:
    def __init__(self, format: object) -> None:
        normalized = _normalize_format(format)
        self.format = normalized
        self.size = calcsize(normalized)

    def pack(self, *values: object) -> bytes:
        return _MOLT_STRUCT_PACK(_STRUCT_ERROR, self.format, values)

    def unpack(self, buffer: object) -> tuple[object, ...]:
        return unpack(self.format, buffer)

    def pack_into(self, buffer: object, offset: int, *values: object) -> None:
        return _MOLT_STRUCT_PACK_INTO(
            _STRUCT_ERROR, self.format, buffer, offset, values
        )

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
    return _MOLT_STRUCT_PACK(_STRUCT_ERROR, _normalize_format(format), values)


def unpack(format: object, buffer: object) -> tuple[object, ...]:
    return _MOLT_STRUCT_UNPACK(_STRUCT_ERROR, _normalize_format(format), buffer)


def calcsize(format: object) -> int:
    return _MOLT_STRUCT_CALCSIZE(_STRUCT_ERROR, _normalize_format(format))


def pack_into(format: object, buffer: object, offset: object, *values: object) -> None:
    return _MOLT_STRUCT_PACK_INTO(
        _STRUCT_ERROR, _normalize_format(format), buffer, offset, values
    )


def unpack_from(
    format: object, buffer: object, offset: object = 0
) -> tuple[object, ...]:
    return _MOLT_STRUCT_UNPACK_FROM(
        _STRUCT_ERROR, _normalize_format(format), buffer, offset
    )


def iter_unpack(format: object, buffer: object):
    return _MOLT_STRUCT_ITER_UNPACK(
        _STRUCT_ERROR, _UNPACK_ITERATOR_TYPE, _normalize_format(format), buffer
    )


globals().pop("_require_intrinsic", None)
