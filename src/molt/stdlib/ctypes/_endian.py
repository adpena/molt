"""Public API surface shim for ``ctypes._endian``."""

from __future__ import annotations


from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")


class PyCSimpleType(type):
    pass


class PyCArrayType(type):
    pass


class PyCStructType(type):
    pass


class UnionType(type):
    pass


class _swapped_struct_meta(type):
    pass


class _swapped_union_meta(type):
    pass


class CFunctionType(type):
    pass


class ArgumentError(Exception):
    pass


class CDLL:
    pass


class PyDLL:
    pass


class LibraryLoader:
    def __init__(self, dlltype):
        self._dlltype = dlltype


class Array(metaclass=PyCArrayType):
    pass


class Structure(metaclass=PyCStructType):
    pass


class LittleEndianStructure(metaclass=PyCStructType):
    pass


class Union(metaclass=UnionType):
    pass


class LittleEndianUnion(metaclass=UnionType):
    pass


class BigEndianStructure(metaclass=_swapped_struct_meta):
    pass


class BigEndianUnion(metaclass=_swapped_union_meta):
    pass


def _make_simple(name: str):
    return PyCSimpleType(name, (), {})


for _name in (
    "c_bool",
    "c_byte",
    "c_char",
    "c_char_p",
    "c_double",
    "c_float",
    "c_int",
    "c_long",
    "c_longdouble",
    "c_longlong",
    "c_short",
    "c_size_t",
    "c_ssize_t",
    "c_ubyte",
    "c_uint",
    "c_ulong",
    "c_ulonglong",
    "c_ushort",
    "c_void_p",
    "c_wchar",
    "c_wchar_p",
    "py_object",
):
    globals()[_name] = _make_simple(_name)

c_voidp = c_void_p


def ARRAY(*_args, **_kwargs):
    return None


def CFUNCTYPE(*_args, **_kwargs):
    return None


def PYFUNCTYPE(*_args, **_kwargs):
    return None


def SetPointerType(*_args, **_kwargs):
    return None


def c_buffer(*_args, **_kwargs):
    return None


def cast(*_args, **_kwargs):
    return None


def create_string_buffer(*_args, **_kwargs):
    return None


def create_unicode_buffer(*_args, **_kwargs):
    return None


def string_at(*_args, **_kwargs):
    return ""


def wstring_at(*_args, **_kwargs):
    return ""


DEFAULT_MODE = 0
RTLD_GLOBAL = 0
RTLD_LOCAL = 0
SIZEOF_TIME_T = 8

cdll = LibraryLoader(CDLL)
pydll = LibraryLoader(PyDLL)
pythonapi = PyDLL()

POINTER = len
addressof = len
alignment = len
byref = len
get_errno = len
pointer = len
resize = len
set_errno = len
sizeof = len

memmove = CFunctionType("memmove", (), {})
memset = CFunctionType("memset", (), {})

del _name
del _make_simple
del PyCSimpleType
del PyCArrayType
del PyCStructType
del UnionType
del CFunctionType

globals().pop("_require_intrinsic", None)
