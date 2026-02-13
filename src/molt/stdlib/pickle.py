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
    buffers=None,
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
    *,
    fix_imports: bool = True,
    encoding: str = "ASCII",
    errors: str = "strict",
    buffers=None,
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
    dispatch_table = _copyreg.dispatch_table

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
        self.memo: dict[int, Any] = {}

    def clear_memo(self) -> None:
        self.memo.clear()

    def persistent_id(self, obj: Any) -> Any | None:
        return None

    def dump(self, obj: Any) -> None:
        try:
            payload = _pickle_dumps_core(
                obj,
                self.protocol,
                self.fix_imports,
                self.persistent_id,
                self.buffer_callback,
                self.dispatch_table,
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
        buffers=None,
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


def _test_roundtrip(obj: Any, protocol: int = DEFAULT_PROTOCOL) -> Any:
    buf = BytesIO()
    dump(obj, buf, protocol=protocol)
    buf.seek(0)
    return load(buf)
