"""Minimal pickle support for Molt (protocols 0 and 1)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from typing import Any

__all__ = [
    "PickleError",
    "PicklingError",
    "UnpicklingError",
    "HIGHEST_PROTOCOL",
    "DEFAULT_PROTOCOL",
    "dump",
    "dumps",
    "load",
    "loads",
]

_require_intrinsic("molt_stdlib_probe", globals())
_pickle_encode_protocol0 = _require_intrinsic("molt_pickle_encode_protocol0", globals())

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): support
# pickle protocols >= 2, memoized cycles, and additional builtins (complex,
# dataclasses, custom reducers, and persistent IDs).


class PickleError(Exception):
    pass


class PicklingError(PickleError):
    pass


class UnpicklingError(PickleError):
    pass


HIGHEST_PROTOCOL = 1
DEFAULT_PROTOCOL = 0

_MARK = object()
_ALLOWED_GLOBALS = {
    ("_codecs", "encode"): lambda text, encoding="utf-8": _codecs_encode(
        text, encoding
    ),
    ("builtins", "bytearray"): bytearray,
    ("__builtin__", "bytearray"): bytearray,
    ("builtins", "slice"): slice,
    ("__builtin__", "slice"): slice,
    ("builtins", "set"): set,
    ("__builtin__", "set"): set,
    ("builtins", "frozenset"): frozenset,
    ("__builtin__", "frozenset"): frozenset,
    ("builtins", "list"): list,
    ("__builtin__", "list"): list,
    ("builtins", "tuple"): tuple,
    ("__builtin__", "tuple"): tuple,
    ("builtins", "dict"): dict,
    ("__builtin__", "dict"): dict,
}


def dump(obj: Any, file, protocol: int | None = None) -> None:
    file.write(dumps(obj, protocol=protocol))


def dumps(obj: Any, protocol: int | None = None) -> bytes:
    if protocol is None:
        protocol = DEFAULT_PROTOCOL
    if protocol not in (0, 1):
        raise ValueError("only pickle protocols 0 and 1 are supported")
    out: list[str] = []
    _dump_obj(obj, out, protocol)
    out.append(".")
    return _pickle_encode_protocol0(out)


def load(file) -> Any:
    return loads(file.read())


def loads(data: bytes | bytearray | str) -> Any:
    if isinstance(data, (bytes, bytearray)):
        text = bytes(data).decode("utf-8")
    else:
        text = data
    stack: list[Any] = []
    memo: dict[int, Any] = {}
    idx = 0
    length = len(text)
    while idx < length:
        op = text[idx]
        idx += 1
        if op == ".":
            break
        if op == "N":
            stack.append(None)
        elif op == "I":
            line, idx = _read_line(text, idx)
            if line == "01":
                stack.append(True)
            elif line == "00":
                stack.append(False)
            else:
                stack.append(int(line))
        elif op == "F":
            line, idx = _read_line(text, idx)
            stack.append(float(line))
        elif op == "S":
            line, idx = _read_line(text, idx)
            stack.append(_parse_string_literal(line))
        elif op == "V":
            line, idx = _read_line(text, idx)
            stack.append(line)
        elif op == "(":
            stack.append(_MARK)
        elif op == "t":
            items = _pop_mark(stack)
            stack.append(tuple(items))
        elif op == "l":
            items = _pop_mark(stack)
            stack.append(list(items))
        elif op == "d":
            items = _pop_mark(stack)
            if len(items) % 2:
                raise UnpicklingError("dict has odd number of items")
            result: dict[Any, Any] = {}
            it = iter(items)
            for key in it:
                result[key] = next(it)
            stack.append(result)
        elif op == "a":
            item = _pop_stack(stack)
            target = _pop_stack(stack)
            if not isinstance(target, list):
                raise UnpicklingError("append target is not list")
            target.append(item)
            stack.append(target)
        elif op == "s":
            value = _pop_stack(stack)
            key = _pop_stack(stack)
            target = _pop_stack(stack)
            if not isinstance(target, dict):
                raise UnpicklingError("setitem target is not dict")
            target[key] = value
            stack.append(target)
        elif op == "c":
            module, idx = _read_line(text, idx)
            name, idx = _read_line(text, idx)
            key = (module, name)
            if key not in _ALLOWED_GLOBALS:
                raise UnpicklingError(f"unsupported global {module}.{name}")
            stack.append(_ALLOWED_GLOBALS[key])
        elif op == "R":
            args = _pop_stack(stack)
            func = _pop_stack(stack)
            if not isinstance(args, tuple):
                raise UnpicklingError("reduce args must be tuple")
            stack.append(_apply_reduce(func, args))
        elif op == "p":
            line, idx = _read_line(text, idx)
            memo[int(line)] = _pop_stack(stack)
            stack.append(memo[int(line)])
        elif op == "g":
            line, idx = _read_line(text, idx)
            key = int(line)
            if key not in memo:
                raise UnpicklingError(f"memo key {key} missing")
            stack.append(memo[key])
        else:
            raise UnpicklingError(f"unsupported opcode {op!r}")
    if not stack:
        raise UnpicklingError("pickle stack empty")
    return stack[-1]


def _read_line(text: str, idx: int) -> tuple[str, int]:
    end = text.find("\n", idx)
    if end < 0:
        raise UnpicklingError("unexpected end of stream")
    return text[idx:end], end + 1


def _pop_mark(stack: list[Any]) -> list[Any]:
    items: list[Any] = []
    while stack:
        item = stack.pop()
        if item is _MARK:
            items.reverse()
            return items
        items.append(item)
    raise UnpicklingError("mark not found")


def _pop_stack(stack: list[Any]) -> Any:
    if not stack:
        raise UnpicklingError("stack underflow")
    return stack.pop()


def _dump_global(module: str, name: str, out: list[str]) -> None:
    out.append(f"c{module}\n{name}\n")


def _dump_obj(obj: Any, out: list[str], protocol: int) -> None:
    if obj is None:
        out.append("N")
        return
    if isinstance(obj, bool):
        out.append("I01\n" if obj else "I00\n")
        return
    if isinstance(obj, int):
        out.append(f"I{obj}\n")
        return
    if isinstance(obj, float):
        out.append(f"F{obj}\n")
        return
    if isinstance(obj, str):
        out.append(f"S{obj!r}\n")
        return
    if isinstance(obj, bytes):
        _dump_global("_codecs", "encode", out)
        _dump_obj((obj.decode("latin1"), "latin1"), out, protocol)
        out.append("R")
        return
    if isinstance(obj, bytearray):
        _dump_global("builtins", "bytearray", out)
        _dump_obj((bytes(obj),), out, protocol)
        out.append("R")
        return
    if isinstance(obj, tuple):
        out.append("(")
        for item in obj:
            _dump_obj(item, out, protocol)
        out.append("t")
        return
    if isinstance(obj, list):
        out.append("(")
        out.append("l")
        for item in obj:
            _dump_obj(item, out, protocol)
            out.append("a")
        return
    if isinstance(obj, dict):
        out.append("(")
        out.append("d")
        for key, value in obj.items():
            _dump_obj(key, out, protocol)
            _dump_obj(value, out, protocol)
            out.append("s")
        return
    if isinstance(obj, set):
        _dump_global("builtins", "set", out)
        out.append("(")
        _dump_obj(list(obj), out, protocol)
        out.append("t")
        out.append("R")
        return
    if isinstance(obj, frozenset):
        _dump_global("builtins", "frozenset", out)
        out.append("(")
        _dump_obj(list(obj), out, protocol)
        out.append("t")
        out.append("R")
        return
    if isinstance(obj, slice):
        _dump_global("builtins", "slice", out)
        _dump_obj((obj.start, obj.stop, obj.step), out, protocol)
        out.append("R")
        return
    raise PicklingError(f"unsupported type: {type(obj).__name__}")


def _codecs_encode(text: str, encoding: str = "utf-8") -> bytes:
    if not isinstance(text, str):
        raise TypeError("text must be str")
    if not isinstance(encoding, str):
        raise TypeError("encoding must be str")
    return text.encode(encoding)


def _parse_string_literal(text: str) -> str:
    if len(text) < 2 or text[0] not in ("'", '"') or text[-1] != text[0]:
        raise UnpicklingError("invalid string literal")
    out: list[str] = []
    idx = 1
    end = len(text) - 1
    while idx < end:
        ch = text[idx]
        if ch != "\\":
            out.append(ch)
            idx += 1
            continue
        idx += 1
        if idx >= end:
            raise UnpicklingError("invalid escape sequence")
        esc = text[idx]
        idx += 1
        if esc == "a":
            out.append("\a")
        elif esc == "b":
            out.append("\b")
        elif esc == "f":
            out.append("\f")
        elif esc == "n":
            out.append("\n")
        elif esc == "r":
            out.append("\r")
        elif esc == "t":
            out.append("\t")
        elif esc == "v":
            out.append("\v")
        elif esc in ("\\", "'", '"'):
            out.append(esc)
        elif esc == "x":
            if idx + 2 > end:
                raise UnpicklingError("invalid hex escape")
            hex_text = text[idx : idx + 2]
            try:
                out.append(chr(int(hex_text, 16)))
            except ValueError as exc:
                raise UnpicklingError("invalid hex escape") from exc
            idx += 2
        elif esc == "u":
            if idx + 4 > end:
                raise UnpicklingError("invalid unicode escape")
            hex_text = text[idx : idx + 4]
            try:
                out.append(chr(int(hex_text, 16)))
            except ValueError as exc:
                raise UnpicklingError("invalid unicode escape") from exc
            idx += 4
        elif esc == "U":
            if idx + 8 > end:
                raise UnpicklingError("invalid unicode escape")
            hex_text = text[idx : idx + 8]
            try:
                out.append(chr(int(hex_text, 16)))
            except ValueError as exc:
                raise UnpicklingError("invalid unicode escape") from exc
            idx += 8
        elif esc in "01234567":
            octal = esc
            limit = min(idx + 2, end)
            while idx < limit and text[idx] in "01234567":
                octal += text[idx]
                idx += 1
            out.append(chr(int(octal, 8)))
        else:
            raise UnpicklingError("invalid escape sequence")
    return "".join(out)


def _apply_reduce(func, args: tuple[Any, ...]) -> Any:
    argc = len(args)
    if argc == 0:
        return func()
    if argc == 1:
        return func(args[0])
    if argc == 2:
        return func(args[0], args[1])
    if argc == 3:
        return func(args[0], args[1], args[2])
    return func(*args)
