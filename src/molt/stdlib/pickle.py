"""Intrinsic-backed pickle support for Molt (protocols 0-5 core)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from importlib import import_module as _import_module
from io import BytesIO
from typing import Any

import copyreg as _copyreg

__all__ = [
    "PickleError",
    "PicklingError",
    "UnpicklingError",
    "PickleBuffer",
    "Pickler",
    "Unpickler",
    "HIGHEST_PROTOCOL",
    "DEFAULT_PROTOCOL",
    "dump",
    "dumps",
    "load",
    "loads",
    "_Pickler",
    "_Unpickler",
    "MARK",
    "STOP",
    "POP",
    "POP_MARK",
    "DUP",
    "FLOAT",
    "INT",
    "BININT",
    "BININT1",
    "LONG",
    "BININT2",
    "NONE",
    "PERSID",
    "BINPERSID",
    "REDUCE",
    "STRING",
    "BINSTRING",
    "SHORT_BINSTRING",
    "UNICODE",
    "BINUNICODE",
    "APPEND",
    "BUILD",
    "GLOBAL",
    "DICT",
    "EMPTY_DICT",
    "APPENDS",
    "GET",
    "BINGET",
    "INST",
    "LONG_BINGET",
    "LIST",
    "EMPTY_LIST",
    "OBJ",
    "PUT",
    "BINPUT",
    "LONG_BINPUT",
    "SETITEM",
    "TUPLE",
    "EMPTY_TUPLE",
    "SETITEMS",
    "BINFLOAT",
    "PROTO",
    "NEWOBJ",
    "EXT1",
    "EXT2",
    "EXT4",
    "TUPLE1",
    "TUPLE2",
    "TUPLE3",
    "NEWTRUE",
    "NEWFALSE",
    "LONG1",
    "LONG4",
    "SHORT_BINBYTES",
    "BINBYTES",
    "BINBYTES8",
    "SHORT_BINUNICODE",
    "BINUNICODE8",
    "EMPTY_SET",
    "ADDITEMS",
    "FROZENSET",
    "NEWOBJ_EX",
    "STACK_GLOBAL",
    "MEMOIZE",
    "FRAME",
    "BYTEARRAY8",
    "NEXT_BUFFER",
    "READONLY_BUFFER",
    "TRUE",
    "FALSE",
]

_require_intrinsic("molt_stdlib_probe", globals())
_pickle_dumps_core = _require_intrinsic("molt_pickle_dumps_core", globals())
_pickle_loads_core = _require_intrinsic("molt_pickle_loads_core", globals())

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): close
# remaining CPython 3.12+ pickle parity gaps (full opcode/PEP edge coverage,
# reducer/API edge semantics, and exact diagnostic-text parity).


class PickleError(Exception):
    pass


class PicklingError(PickleError):
    pass


class UnpicklingError(PickleError):
    pass


class PickleBuffer:
    __molt_pickle_buffer__ = True

    def __init__(self, buffer) -> None:
        self._buffer = buffer

    def raw(self):
        return memoryview(self._buffer)

    def release(self) -> None:
        self._buffer = b""


HIGHEST_PROTOCOL = 5
DEFAULT_PROTOCOL = 4

# pickle opcode constants (CPython-compatible exported surface).
MARK = b"("
STOP = b"."
POP = b"0"
POP_MARK = b"1"
DUP = b"2"
FLOAT = b"F"
INT = b"I"
BININT = b"J"
BININT1 = b"K"
LONG = b"L"
BININT2 = b"M"
NONE = b"N"
PERSID = b"P"
BINPERSID = b"Q"
REDUCE = b"R"
STRING = b"S"
BINSTRING = b"T"
SHORT_BINSTRING = b"U"
UNICODE = b"V"
BINUNICODE = b"X"
APPEND = b"a"
BUILD = b"b"
GLOBAL = b"c"
DICT = b"d"
EMPTY_DICT = b"}"
APPENDS = b"e"
GET = b"g"
BINGET = b"h"
INST = b"i"
LONG_BINGET = b"j"
LIST = b"l"
EMPTY_LIST = b"]"
OBJ = b"o"
PUT = b"p"
BINPUT = b"q"
LONG_BINPUT = b"r"
SETITEM = b"s"
TUPLE = b"t"
EMPTY_TUPLE = b")"
SETITEMS = b"u"
BINFLOAT = b"G"
PROTO = b"\x80"
NEWOBJ = b"\x81"
EXT1 = b"\x82"
EXT2 = b"\x83"
EXT4 = b"\x84"
TUPLE1 = b"\x85"
TUPLE2 = b"\x86"
TUPLE3 = b"\x87"
NEWTRUE = b"\x88"
NEWFALSE = b"\x89"
LONG1 = b"\x8a"
LONG4 = b"\x8b"
SHORT_BINUNICODE = b"\x8c"
BINUNICODE8 = b"\x8d"
BINBYTES8 = b"\x8e"
EMPTY_SET = b"\x8f"
ADDITEMS = b"\x90"
FROZENSET = b"\x91"
NEWOBJ_EX = b"\x92"
STACK_GLOBAL = b"\x93"
MEMOIZE = b"\x94"
FRAME = b"\x95"
BYTEARRAY8 = b"\x96"
NEXT_BUFFER = b"\x97"
READONLY_BUFFER = b"\x98"
SHORT_BINBYTES = b"C"
BINBYTES = b"B"
TRUE = b"I01\n"
FALSE = b"I00\n"


def _normalize_protocol(protocol: int | None) -> int:
    if protocol is None:
        return DEFAULT_PROTOCOL
    if protocol < -1 or protocol > HIGHEST_PROTOCOL:
        raise ValueError("pickle protocol must be in range -1..5")
    if protocol == -1:
        return HIGHEST_PROTOCOL
    return protocol


def dump(
    obj: Any,
    file,
    protocol: int | None = None,
    *,
    fix_imports: bool = True,
    buffer_callback=None,
) -> None:
    file.write(
        dumps(
            obj,
            protocol=protocol,
            fix_imports=fix_imports,
            buffer_callback=buffer_callback,
        )
    )


def dumps(
    obj: Any,
    protocol: int | None = None,
    *,
    fix_imports: bool = True,
    buffer_callback=None,
) -> bytes:
    normalized = _normalize_protocol(protocol)
    try:
        return _pickle_dumps_core(
            obj,
            normalized,
            bool(fix_imports),
            None,
            buffer_callback,
            _copyreg.dispatch_table,
        )
    except RuntimeError as exc:
        raise PicklingError(str(exc)) from exc


def load(
    file,
    *,
    fix_imports: bool = True,
    encoding: str = "ASCII",
    errors: str = "strict",
    buffers=(),
) -> Any:
    return Unpickler(
        file,
        fix_imports=fix_imports,
        encoding=encoding,
        errors=errors,
        buffers=buffers,
    ).load()


def loads(
    data: bytes | bytearray | str,
    /,
    *,
    fix_imports: bool = True,
    encoding: str = "ASCII",
    errors: str = "strict",
    buffers=(),
) -> Any:
    try:
        return _pickle_loads_core(
            data,
            bool(fix_imports),
            encoding,
            errors,
            None,
            None,
            buffers,
        )
    except RuntimeError as exc:
        raise UnpicklingError(str(exc)) from exc


class Pickler:
    def __init__(
        self,
        file,
        protocol: int | None = None,
        *,
        fix_imports: bool = True,
        buffer_callback=None,
    ) -> None:
        self._file = file
        self.protocol = _normalize_protocol(protocol)
        self.fix_imports = bool(fix_imports)
        self.buffer_callback = buffer_callback
        self.bin = 0 if self.protocol == 0 else 1
        self.fast = 0
        self.memo: dict[int, Any] = {}

    def clear_memo(self) -> None:
        self.memo.clear()

    def persistent_id(self, obj: Any) -> Any | None:
        return None

    def dump(self, obj: Any) -> None:
        dispatch_table = getattr(self, "dispatch_table", _copyreg.dispatch_table)
        try:
            payload = _pickle_dumps_core(
                obj,
                self.protocol,
                self.fix_imports,
                self.persistent_id,
                self.buffer_callback,
                dispatch_table,
            )
        except RuntimeError as exc:
            raise PicklingError(str(exc)) from exc
        self._file.write(payload)


class Unpickler:
    def __init__(
        self,
        file,
        *,
        fix_imports: bool = True,
        encoding: str = "ASCII",
        errors: str = "strict",
        buffers=(),
    ) -> None:
        self._file = file
        self.fix_imports = bool(fix_imports)
        self.encoding = encoding
        self.errors = errors
        self.buffers = buffers

    def persistent_load(self, pid):
        raise UnpicklingError("unsupported persistent id encountered")

    def find_class(self, module: str, name: str):
        mod = _import_module(module)
        return getattr(mod, name)

    def load(self) -> Any:
        data = self._file.read()
        try:
            return _pickle_loads_core(
                data,
                self.fix_imports,
                self.encoding,
                self.errors,
                self.persistent_load,
                self.find_class,
                self.buffers,
            )
        except RuntimeError as exc:
            raise UnpicklingError(str(exc)) from exc


def _dump(obj: Any, file, protocol: int | None = None) -> None:
    dump(obj, file, protocol=protocol)


def _dumps(obj: Any, protocol: int | None = None) -> bytes:
    return dumps(obj, protocol=protocol)


def _load(file) -> Any:
    return load(file)


def _loads(data: bytes | bytearray | str) -> Any:
    return loads(data)


_Pickler = Pickler
_Unpickler = Unpickler


def _test_roundtrip(obj: Any, protocol: int = DEFAULT_PROTOCOL) -> Any:
    buf = BytesIO()
    dump(obj, buf, protocol=protocol)
    buf.seek(0)
    return load(buf)
