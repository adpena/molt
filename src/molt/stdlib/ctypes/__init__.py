"""Intrinsic-backed ctypes subset for Molt (ffi.unsafe capability-gated)."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "Array",
    "_SimpleCData",
    "Structure",
    "c_bool",
    "c_byte",
    "c_char",
    "c_double",
    "c_float",
    "c_int8",
    "c_int16",
    "c_int32",
    "c_int64",
    "c_int",
    "c_long",
    "c_longlong",
    "c_short",
    "c_size_t",
    "c_ubyte",
    "c_uint8",
    "c_uint16",
    "c_uint32",
    "c_uint64",
    "c_uint",
    "c_ulong",
    "c_ulonglong",
    "c_ushort",
    "c_void_p",
    "pointer",
    "sizeof",
]

_MOLT_CTYPES_REQUIRE_FFI = _require_intrinsic("molt_ctypes_require_ffi")
_MOLT_CTYPES_COERCE_VALUE = _require_intrinsic("molt_ctypes_coerce_value")
_MOLT_CTYPES_DEFAULT_VALUE = _require_intrinsic("molt_ctypes_default_value")
_MOLT_CTYPES_SIZEOF = _require_intrinsic("molt_ctypes_sizeof")

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): expand ctypes intrinsic coverage beyond the core scalar/structure/array/pointer subset.


def _require_ffi() -> None:
    _MOLT_CTYPES_REQUIRE_FFI()


class _CType:
    _size = 0

    def __init__(self, value: Any = 0) -> None:
        self.value = value

    def __int__(self) -> int:
        return int(self.value)

    def __repr__(self) -> str:
        return f"{self.__class__.__name__}({self.value!r})"


_SimpleCData = _CType


class _CTypeSpec:
    def __init__(
        self,
        name: str,
        size: int,
        *,
        kind: str = "int",
        bits: int | None = None,
        signed: bool = True,
    ) -> None:
        self.__name__ = name
        self._size = int(size)
        self._kind = kind
        self._bits = int(bits if bits is not None else size * 8)
        self._signed = bool(signed)

    def __call__(self, value: Any = 0) -> _CType:
        _MOLT_CTYPES_REQUIRE_FFI()
        inst = _CType(_MOLT_CTYPES_COERCE_VALUE(self, value))
        inst._size = self._size
        inst._kind = self._kind
        inst._bits = self._bits
        inst._signed = self._signed
        return inst

    def __mul__(self, length: int) -> type:
        return _make_array_type(self, length)

    def __rmul__(self, length: int) -> type:
        return _make_array_type(self, length)

    def __repr__(self) -> str:
        return self.__name__


c_bool = _CTypeSpec("c_bool", 1, kind="bool", bits=8, signed=False)
c_char = _CTypeSpec("c_char", 1, kind="char", bits=8, signed=False)
c_byte = _CTypeSpec("c_byte", 1, bits=8, signed=True)
c_ubyte = _CTypeSpec("c_ubyte", 1, bits=8, signed=False)
c_int8 = _CTypeSpec("c_int8", 1, bits=8, signed=True)
c_uint8 = _CTypeSpec("c_uint8", 1, bits=8, signed=False)
c_int16 = _CTypeSpec("c_int16", 2, bits=16, signed=True)
c_uint16 = _CTypeSpec("c_uint16", 2, bits=16, signed=False)
c_short = _CTypeSpec("c_short", 2, bits=16, signed=True)
c_ushort = _CTypeSpec("c_ushort", 2, bits=16, signed=False)
c_int = _CTypeSpec("c_int", 4, bits=32, signed=True)
c_uint = _CTypeSpec("c_uint", 4, bits=32, signed=False)
c_int32 = _CTypeSpec("c_int32", 4, bits=32, signed=True)
c_uint32 = _CTypeSpec("c_uint32", 4, bits=32, signed=False)
c_long = _CTypeSpec("c_long", 8, bits=64, signed=True)
c_ulong = _CTypeSpec("c_ulong", 8, bits=64, signed=False)
c_longlong = _CTypeSpec("c_longlong", 8, bits=64, signed=True)
c_ulonglong = _CTypeSpec("c_ulonglong", 8, bits=64, signed=False)
c_int64 = _CTypeSpec("c_int64", 8, bits=64, signed=True)
c_uint64 = _CTypeSpec("c_uint64", 8, bits=64, signed=False)
c_size_t = _CTypeSpec("c_size_t", 8, bits=64, signed=False)
c_void_p = _CTypeSpec("c_void_p", 8, kind="void_p", bits=64, signed=False)
c_float = _CTypeSpec("c_float", 4, kind="float", bits=32, signed=True)
c_double = _CTypeSpec("c_double", 8, kind="float", bits=64, signed=True)


def _coerce_value(ctype: Any, value: Any) -> Any:
    _MOLT_CTYPES_REQUIRE_FFI()
    return _MOLT_CTYPES_COERCE_VALUE(ctype, value)


def _default_value(ctype: Any) -> Any:
    _MOLT_CTYPES_REQUIRE_FFI()
    return _MOLT_CTYPES_DEFAULT_VALUE(ctype)


def _sizeof_type(ctype: Any) -> int:
    _MOLT_CTYPES_REQUIRE_FFI()
    out = _MOLT_CTYPES_SIZEOF(ctype)
    if not isinstance(out, int):
        raise RuntimeError("ctypes sizeof intrinsic returned invalid value")
    return out


def _make_array_type(ctype: Any, length: int) -> type:
    _require_ffi()
    try:
        length_val = int(length)
    except Exception as exc:  # pragma: no cover - guard rail
        raise TypeError("array length must be int") from exc
    if length_val < 0:
        raise ValueError("array length must be non-negative")

    class Array(_ArrayBase):
        _ctype = ctype
        _length = length_val
        _size = _sizeof_type(ctype) * length_val

        def __init__(self, *values: Any) -> None:
            if len(values) > length_val:
                raise ValueError("too many initializers")
            items = [_coerce_value(ctype, val) for val in values]
            if len(items) < length_val:
                items.extend(
                    _default_value(ctype) for _ in range(length_val - len(items))
                )
            self._items = items

        def __len__(self) -> int:
            return length_val

        def __iter__(self) -> Any:
            return iter(self._items)

        def __getitem__(self, idx: int) -> Any:
            return self._items[idx]

        def __setitem__(self, idx: int, value: Any) -> None:
            self._items[idx] = _coerce_value(ctype, value)

        def __repr__(self) -> str:
            return f"{ctype.__name__} * {length_val}"

    Array.__name__ = f"{ctype.__name__}Array_{length_val}"
    return Array


class _ArrayBase:
    _ctype: Any
    _length = 0
    _size = 0


Array = _ArrayBase


class _StructureMeta(type):
    def __new__(mcls, name: str, bases: tuple[type, ...], namespace: dict[str, Any]):
        cls = super().__new__(mcls, name, bases, namespace)
        fields = list(getattr(cls, "_fields_", []))
        if fields:
            size = 0
            for _, field_type in fields:
                size += _sizeof_type(field_type)
            cls._size = size
        return cls


class Structure(metaclass=_StructureMeta):
    _fields_: list[tuple[str, Any]] = []
    _size = 0

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        _require_ffi()
        fields = list(self.__class__._fields_)
        if len(args) > len(fields):
            raise TypeError("too many initializers")
        for index, (name, field_type) in enumerate(fields):
            if index < len(args):
                value = args[index]
            elif name in kwargs:
                value = kwargs[name]
            else:
                value = _default_value(field_type)
            setattr(self, name, _coerce_value(field_type, value))


class _Pointer:
    def __init__(self, obj: Any) -> None:
        _require_ffi()
        self._obj = obj

    @property
    def contents(self) -> Any:
        return self._obj


def pointer(obj: Any) -> _Pointer:
    _require_ffi()
    return _Pointer(obj)


def sizeof(obj_or_type: Any) -> int:
    _require_ffi()
    return _sizeof_type(obj_or_type)


globals().pop("_require_intrinsic", None)
