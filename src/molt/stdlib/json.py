"""Minimal JSON shim for Molt."""

from __future__ import annotations

import math
from typing import Any, Callable

__all__ = [
    "dump",
    "dumps",
    "load",
    "loads",
    "JSONDecodeError",
]

JSONDecodeError = ValueError

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement full json parity (JSONEncoder/Decoder classes, JSONDecodeError details, encoding options, and performance tuning).


def loads(
    s: str | bytes | bytearray,
    *,
    cls: Any | None = None,
    object_hook: Callable[[dict[str, Any]], Any] | None = None,
    parse_float: Callable[[str], Any] | None = None,
    parse_int: Callable[[str], Any] | None = None,
    parse_constant: Callable[[str], Any] | None = None,
    object_pairs_hook: Callable[[list[tuple[str, Any]]], Any] | None = None,
    **_kwargs: Any,
) -> Any:
    _ = cls
    if isinstance(s, (bytes, bytearray)):
        text = s.decode("utf-8")
    elif isinstance(s, str):
        text = s
    else:
        raise TypeError(
            f"the JSON object must be str, bytes or bytearray, not {type(s).__name__}"
        )
    parser = _Parser(
        text,
        parse_float=parse_float,
        parse_int=parse_int,
        parse_constant=parse_constant,
        object_hook=object_hook,
        object_pairs_hook=object_pairs_hook,
    )
    return parser.parse()


def load(
    fp: Any,
    *,
    cls: Any | None = None,
    object_hook: Callable[[dict[str, Any]], Any] | None = None,
    parse_float: Callable[[str], Any] | None = None,
    parse_int: Callable[[str], Any] | None = None,
    parse_constant: Callable[[str], Any] | None = None,
    object_pairs_hook: Callable[[list[tuple[str, Any]]], Any] | None = None,
    **_kwargs: Any,
) -> Any:
    return loads(
        fp.read(),
        cls=cls,
        object_hook=object_hook,
        parse_float=parse_float,
        parse_int=parse_int,
        parse_constant=parse_constant,
        object_pairs_hook=object_pairs_hook,
    )


def dumps(
    obj: Any,
    *,
    skipkeys: bool = False,
    ensure_ascii: bool = True,
    check_circular: bool = True,
    allow_nan: bool = True,
    sort_keys: bool = False,
    indent: int | str | None = None,
    separators: tuple[str, str] | None = None,
    default: Callable[[Any], Any] | None = None,
) -> str:
    if separators is None:
        separators = (", ", ": ") if indent is None else (",", ": ")
    elif len(separators) != 2:
        raise ValueError("separators must be a (item, key) tuple")
    enc = _Encoder(
        skipkeys=skipkeys,
        ensure_ascii=ensure_ascii,
        check_circular=check_circular,
        allow_nan=allow_nan,
        sort_keys=sort_keys,
        indent=indent,
        separators=separators,
        default=default,
    )
    return enc.encode(obj)


def dump(
    obj: Any,
    fp: Any,
    *,
    skipkeys: bool = False,
    ensure_ascii: bool = True,
    check_circular: bool = True,
    allow_nan: bool = True,
    sort_keys: bool = False,
    indent: int | str | None = None,
    separators: tuple[str, str] | None = None,
    default: Callable[[Any], Any] | None = None,
) -> None:
    text = dumps(
        obj,
        skipkeys=skipkeys,
        ensure_ascii=ensure_ascii,
        check_circular=check_circular,
        allow_nan=allow_nan,
        sort_keys=sort_keys,
        indent=indent,
        separators=separators,
        default=default,
    )
    fp.write(text)


class _Encoder:
    def __init__(
        self,
        *,
        skipkeys: bool,
        ensure_ascii: bool,
        check_circular: bool,
        allow_nan: bool,
        sort_keys: bool,
        indent: int | str | None,
        separators: tuple[str, str],
        default: Callable[[Any], Any] | None,
    ) -> None:
        self.skipkeys = skipkeys
        self.ensure_ascii = ensure_ascii
        self.check_circular = check_circular
        self.allow_nan = allow_nan
        self.sort_keys = sort_keys
        self.indent = indent
        self.sep_item, self.sep_key = separators
        self.default = default
        self._stack: list[int] = []

    def encode(self, obj: Any) -> str:
        return self._encode(obj, 0)

    def _encode(self, obj: Any, depth: int) -> str:
        if obj is None:
            return "null"
        if obj is True:
            return "true"
        if obj is False:
            return "false"
        if isinstance(obj, int) and not isinstance(obj, bool):
            return str(obj)
        if isinstance(obj, float):
            return self._encode_float(obj)
        if isinstance(obj, str):
            return self._encode_str(obj)
        if isinstance(obj, (list, tuple)):
            return self._encode_list(obj, depth)
        if isinstance(obj, dict):
            return self._encode_dict(obj, depth)
        if self.default is not None:
            return self._encode(self.default(obj), depth)
        raise TypeError(f"Object of type {type(obj).__name__} is not JSON serializable")

    def _encode_float(self, value: float) -> str:
        if math.isfinite(value):
            return repr(value)
        if not self.allow_nan:
            raise ValueError("Out of range float values are not JSON compliant")
        if math.isnan(value):
            return "NaN"
        if value > 0:
            return "Infinity"
        return "-Infinity"

    def _encode_str(self, value: str) -> str:
        out = ['"']
        for ch in value:
            code = ord(ch)
            if ch == '"':
                out.append('\\"')
            elif ch == "\\":
                out.append("\\\\")
            elif ch == "\b":
                out.append("\\b")
            elif ch == "\f":
                out.append("\\f")
            elif ch == "\n":
                out.append("\\n")
            elif ch == "\r":
                out.append("\\r")
            elif ch == "\t":
                out.append("\\t")
            elif code < 0x20 or (self.ensure_ascii and code > 0x7E):
                out.append(self._escape_codepoint(code))
            else:
                out.append(ch)
        out.append('"')
        return "".join(out)

    def _escape_codepoint(self, code: int) -> str:
        if code <= 0xFFFF:
            return f"\\u{code:04x}"
        code -= 0x10000
        high = 0xD800 + (code >> 10)
        low = 0xDC00 + (code & 0x3FF)
        return f"\\u{high:04x}\\u{low:04x}"

    def _encode_list(self, items: list[Any] | tuple[Any, ...], depth: int) -> str:
        if self._push(items):
            raise ValueError("Circular reference detected")
        try:
            if not items:
                return "[]"
            pieces: list[str] = []
            for item in items:
                pieces.append(self._encode(item, depth + 1))
            if self.indent is None:
                return "[" + self.sep_item.join(pieces) + "]"
            return self._format_block(pieces, depth, "[", "]")
        finally:
            self._pop(items)

    def _encode_dict(self, mapping: dict[Any, Any], depth: int) -> str:
        if self._push(mapping):
            raise ValueError("Circular reference detected")
        try:
            if not mapping:
                return "{}"
            items = list(mapping.items())
            if self.sort_keys:
                items.sort()
            entries: list[tuple[str, Any]] = []
            for key, value in items:
                key_text = self._key_to_text(key)
                if key_text is None:
                    if self.skipkeys:
                        continue
                    typename = type(key).__name__
                    raise TypeError(
                        f"keys must be str, int, float, bool or None, not {typename}"
                    )
                entries.append((key_text, value))
            pieces: list[str] = []
            for key_text, value in entries:
                encoded_key = self._encode_str(key_text)
                encoded_val = self._encode(value, depth + 1)
                pieces.append(encoded_key + self.sep_key + encoded_val)
            if self.indent is None:
                return "{" + self.sep_item.join(pieces) + "}"
            return self._format_block(pieces, depth, "{", "}")
        finally:
            self._pop(mapping)

    def _format_block(
        self, pieces: list[str], depth: int, open_ch: str, close_ch: str
    ) -> str:
        pad = self._indent_text()
        lines: list[str] = []
        for idx, piece in enumerate(pieces):
            suffix = self.sep_item if idx < len(pieces) - 1 else ""
            lines.append(pad * (depth + 1) + piece + suffix)
        inner = "\n".join(lines)
        return f"{open_ch}\n{inner}\n{pad * depth}{close_ch}"

    def _indent_text(self) -> str:
        if self.indent is None:
            return ""
        if isinstance(self.indent, int):
            return " " * self.indent
        return self.indent

    def _key_to_text(self, key: Any) -> str | None:
        if isinstance(key, str):
            return key
        if isinstance(key, bool):
            return "true" if key else "false"
        if key is None:
            return "null"
        if isinstance(key, int):
            return str(key)
        if isinstance(key, float):
            return self._encode_float(key)
        return None

    def _push(self, obj: Any) -> bool:
        if not self.check_circular:
            return False
        marker = id(obj)
        if marker in self._stack:
            return True
        self._stack.append(marker)
        return False

    def _pop(self, obj: Any) -> None:
        if not self.check_circular:
            return
        marker = id(obj)
        if self._stack and self._stack[-1] == marker:
            self._stack.pop()


class _Parser:
    def __init__(
        self,
        text: str,
        *,
        parse_float: Callable[[str], Any] | None,
        parse_int: Callable[[str], Any] | None,
        parse_constant: Callable[[str], Any] | None,
        object_hook: Callable[[dict[str, Any]], Any] | None,
        object_pairs_hook: Callable[[list[tuple[str, Any]]], Any] | None,
    ) -> None:
        self.text = text
        self.length = len(text)
        self.index = 0
        self.parse_float = parse_float
        self.parse_int = parse_int
        self.parse_constant = parse_constant
        self.object_hook = object_hook
        self.object_pairs_hook = object_pairs_hook

    def parse(self) -> Any:
        self._consume_ws()
        value = self._parse_value()
        self._consume_ws()
        if self.index != self.length:
            raise ValueError("Extra data")
        return value

    def _consume_ws(self) -> None:
        while self.index < self.length and self.text[self.index] in " \t\r\n":
            self.index += 1

    def _peek(self) -> str:
        if self.index >= self.length:
            return ""
        return self.text[self.index]

    def _advance(self) -> str:
        ch = self._peek()
        self.index += 1
        return ch

    def _parse_value(self) -> Any:
        ch = self._peek()
        if ch == "{":
            return self._parse_object()
        if ch == "[":
            return self._parse_array()
        if ch == '"':
            return self._parse_string()
        if ch == "t":
            return self._parse_literal("true", True)
        if ch == "f":
            return self._parse_literal("false", False)
        if ch == "n":
            return self._parse_literal("null", None)
        if ch == "N":
            return self._parse_constant("NaN")
        if ch == "I":
            return self._parse_constant("Infinity")
        if ch == "-" and self._text_startswith("-Infinity"):
            return self._parse_constant("-Infinity")
        if ch == "-" or self._is_digit(ch):
            return self._parse_number()
        raise ValueError("Expecting value")

    def _parse_literal(self, text: str, value: Any) -> Any:
        if self._text_startswith(text):
            self.index += len(text)
            return value
        raise ValueError("Expecting value")

    def _parse_constant(self, text: str) -> Any:
        if not self._text_startswith(text):
            raise ValueError("Expecting value")
        self.index += len(text)
        if self.parse_constant is not None:
            return self.parse_constant(text)
        if text == "NaN":
            return float("nan")
        if text == "Infinity":
            return float("inf")
        return float("-inf")

    def _parse_number(self) -> Any:
        start = self.index
        if self._peek() == "-":
            self.index += 1
        if self.index >= self.length:
            raise ValueError("Expecting value")
        ch = self.text[self.index]
        if ch == "0":
            self.index += 1
        elif self._is_digit(ch):
            while self.index < self.length and self._is_digit(self.text[self.index]):
                self.index += 1
        else:
            raise ValueError("Expecting value")
        if self.index < self.length and self.text[self.index] == ".":
            self.index += 1
            if self.index >= self.length or not self._is_digit(self.text[self.index]):
                raise ValueError("Expecting value")
            while self.index < self.length and self._is_digit(self.text[self.index]):
                self.index += 1
        if self.index < self.length and self.text[self.index] in "eE":
            self.index += 1
            if self.index < self.length and self.text[self.index] in "+-":
                self.index += 1
            if self.index >= self.length or not self._is_digit(self.text[self.index]):
                raise ValueError("Expecting value")
            while self.index < self.length and self._is_digit(self.text[self.index]):
                self.index += 1
        raw = self.text[start : self.index]
        if "." in raw or "e" in raw or "E" in raw:
            if self.parse_float is not None:
                return self.parse_float(raw)
            return float(raw)
        if self.parse_int is not None:
            return self.parse_int(raw)
        return int(raw)

    def _parse_string(self) -> str:
        if self._advance() != '"':
            raise ValueError("Expecting value")
        out: list[str] = []
        while self.index < self.length:
            ch = self._advance()
            if ch == '"':
                return "".join(out)
            if ch == "\\":
                if self.index >= self.length:
                    raise ValueError("Invalid \\uXXXX escape")
                esc = self._advance()
                if esc == '"':
                    out.append('"')
                elif esc == "\\":
                    out.append("\\")
                elif esc == "/":
                    out.append("/")
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
                elif esc == "u":
                    out.append(self._parse_unicode_escape())
                else:
                    raise ValueError("Invalid \\uXXXX escape")
            else:
                if ord(ch) < 0x20:
                    raise ValueError("Invalid control character")
                out.append(ch)
        raise ValueError("Unterminated string starting at")

    def _parse_unicode_escape(self) -> str:
        if self.index + 4 > self.length:
            raise ValueError("Invalid \\uXXXX escape")
        hex_text = self.text[self.index : self.index + 4]
        if not _is_hex(hex_text):
            raise ValueError("Invalid \\uXXXX escape")
        code = int(hex_text, 16)
        self.index += 4
        if 0xD800 <= code <= 0xDBFF:
            if not self._text_startswith("\\u"):
                raise ValueError("Invalid \\uXXXX escape")
            self.index += 2
            if self.index + 4 > self.length:
                raise ValueError("Invalid \\uXXXX escape")
            low_text = self.text[self.index : self.index + 4]
            if not _is_hex(low_text):
                raise ValueError("Invalid \\uXXXX escape")
            low = int(low_text, 16)
            self.index += 4
            if not 0xDC00 <= low <= 0xDFFF:
                raise ValueError("Invalid \\uXXXX escape")
            combined = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00)
            return chr(combined)
        if 0xDC00 <= code <= 0xDFFF:
            raise ValueError("Invalid \\uXXXX escape")
        return chr(code)

    def _parse_array(self) -> list[Any]:
        if self._advance() != "[":
            raise ValueError("Expecting value")
        items: list[Any] = []
        self._consume_ws()
        if self._peek() == "]":
            self.index += 1
            return items
        while True:
            self._consume_ws()
            items.append(self._parse_value())
            self._consume_ws()
            ch = self._advance()
            if ch == "]":
                break
            if ch != ",":
                raise ValueError("Expecting ',' delimiter")
        return items

    def _parse_object(self) -> Any:
        if self._advance() != "{":
            raise ValueError("Expecting value")
        pairs: list[tuple[str, Any]] = []
        self._consume_ws()
        if self._peek() == "}":
            self.index += 1
            return self._finish_object(pairs)
        while True:
            self._consume_ws()
            if self._peek() != '"':
                raise ValueError("Expecting property name enclosed in double quotes")
            key = self._parse_string()
            self._consume_ws()
            if self._advance() != ":":
                raise ValueError("Expecting ':' delimiter")
            self._consume_ws()
            value = self._parse_value()
            pairs.append((key, value))
            self._consume_ws()
            ch = self._advance()
            if ch == "}":
                break
            if ch != ",":
                raise ValueError("Expecting ',' delimiter")
        return self._finish_object(pairs)

    @staticmethod
    def _is_digit(ch: str) -> bool:
        return "0" <= ch <= "9"

    def _text_startswith(self, text: str) -> bool:
        end = self.index + len(text)
        if end > self.length:
            return False
        return self.text[self.index : end] == text

    def _finish_object(self, pairs: list[tuple[str, Any]]) -> Any:
        if self.object_pairs_hook is not None:
            return self.object_pairs_hook(pairs)
        obj: dict[str, Any] = {}
        for key, value in pairs:
            obj[key] = value
        if self.object_hook is not None:
            return self.object_hook(obj)
        return obj


def _is_hex(text: str) -> bool:
    for ch in text:
        if ch not in "0123456789abcdefABCDEF":
            return False
    return True
