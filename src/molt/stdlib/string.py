"""String constants and helpers for Molt."""

from __future__ import annotations

from typing import Any, NoReturn, cast

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())


__all__ = [
    "ascii_letters",
    "ascii_lowercase",
    "ascii_uppercase",
    "digits",
    "hexdigits",
    "octdigits",
    "punctuation",
    "whitespace",
    "printable",
    "capwords",
    "Template",
    "Formatter",
]

ascii_lowercase = "abcdefghijklmnopqrstuvwxyz"
ascii_uppercase = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
ascii_letters = ascii_lowercase + ascii_uppercase
digits = "0123456789"
hexdigits = digits + "abcdef" + "ABCDEF"
octdigits = "01234567"
punctuation = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~"
whitespace = " \t\n\r\x0b\x0c"
printable = digits + ascii_letters + punctuation + whitespace

_sentinel_dict: dict[str, object] = {}


def _is_identifier_start(ch: str) -> bool:
    if not ch:
        return False
    code = ord(ch)
    return ch == "_" or (65 <= code <= 90) or (97 <= code <= 122)


def _is_identifier_continue(ch: str) -> bool:
    if _is_identifier_start(ch):
        return True
    code = ord(ch)
    return 48 <= code <= 57


def _scan_identifier(text: str, start: int) -> tuple[str, int] | None:
    if start >= len(text) or not _is_identifier_start(text[start]):
        return None
    end = start + 1
    while end < len(text) and _is_identifier_continue(text[end]):
        end += 1
    return text[start:end], end


class Template:
    """A string class for supporting $-substitutions."""

    delimiter = "$"
    idpattern = r"(?a:[_a-z][_a-z0-9]*)"
    braceidpattern = None
    flags = None

    def __init__(self, template: str) -> None:
        self.template = template

    def _invalid(self, index: int) -> NoReturn:
        lines = self.template[:index].splitlines(keepends=True)
        if not lines:
            colno = 1
            lineno = 1
        else:
            colno = index - len("".join(lines[:-1]))
            lineno = len(lines)
        raise ValueError(f"Invalid placeholder in string: line {lineno}, col {colno}")

    def _substitute(self, mapping: object, *, safe: bool) -> str:
        text = self.template
        delim = self.delimiter
        if not delim:
            return text
        delim_len = len(delim)
        out: list[str] = []
        idx = 0
        length = len(text)
        while idx < length:
            next_idx = text.find(delim, idx)
            if next_idx == -1:
                out.append(text[idx:])
                break
            out.append(text[idx:next_idx])
            if next_idx + delim_len > length - 1:
                if safe:
                    out.append(delim)
                    break
                self._invalid(next_idx + delim_len)
            next_char = text[next_idx + delim_len]
            if text.startswith(delim, next_idx + delim_len):
                out.append(delim)
                idx = next_idx + delim_len * 2
                continue
            if next_char == "{":
                brace_start = next_idx + delim_len + 1
                brace_end = text.find("}", brace_start)
                if brace_end == -1:
                    if safe:
                        out.append(text[next_idx : next_idx + delim_len])
                        idx = next_idx + delim_len
                        continue
                    self._invalid(next_idx + delim_len)
                name = text[brace_start:brace_end]
                if not name or _scan_identifier(name, 0) is None:
                    if safe:
                        out.append(text[next_idx : next_idx + delim_len])
                        idx = next_idx + delim_len
                        continue
                    self._invalid(next_idx + delim_len)
                if safe:
                    try:
                        out.append(str(mapping[name]))  # type: ignore[index]
                    except KeyError:
                        out.append(text[next_idx : brace_end + 1])
                else:
                    out.append(str(mapping[name]))  # type: ignore[index]
                idx = brace_end + 1
                continue
            ident = _scan_identifier(text, next_idx + delim_len)
            if ident is None:
                if safe:
                    out.append(text[next_idx : next_idx + delim_len])
                    idx = next_idx + delim_len
                    continue
                self._invalid(next_idx + delim_len)
            assert ident is not None
            name, end = ident
            if safe:
                try:
                    out.append(str(mapping[name]))  # type: ignore[index]
                except KeyError:
                    out.append(text[next_idx:end])
            else:
                out.append(str(mapping[name]))  # type: ignore[index]
            idx = end
        return "".join(out)

    def substitute(self, mapping: object = _sentinel_dict, /, **kws) -> str:
        if mapping is _sentinel_dict:
            mapping = kws
        elif kws:
            from collections import ChainMap

            mapping = ChainMap(kws, mapping)  # type: ignore[arg-type]
        return self._substitute(mapping, safe=False)

    def safe_substitute(self, mapping: object = _sentinel_dict, /, **kws) -> str:
        if mapping is _sentinel_dict:
            mapping = kws
        elif kws:
            from collections import ChainMap

            mapping = ChainMap(kws, mapping)  # type: ignore[arg-type]
        return self._substitute(mapping, safe=True)

    def is_valid(self) -> bool:
        text = self.template
        delim = self.delimiter
        if not delim:
            return True
        delim_len = len(delim)
        idx = 0
        length = len(text)
        while idx < length:
            next_idx = text.find(delim, idx)
            if next_idx == -1:
                return True
            if next_idx + delim_len > length - 1:
                return False
            next_char = text[next_idx + delim_len]
            if text.startswith(delim, next_idx + delim_len):
                idx = next_idx + delim_len * 2
                continue
            if next_char == "{":
                brace_start = next_idx + delim_len + 1
                brace_end = text.find("}", brace_start)
                if brace_end == -1:
                    return False
                name = text[brace_start:brace_end]
                if not name or _scan_identifier(name, 0) is None:
                    return False
                idx = brace_end + 1
                continue
            if _scan_identifier(text, next_idx + delim_len) is None:
                return False
            _, end = _scan_identifier(text, next_idx + delim_len)  # type: ignore[misc]
            idx = end
        return True

    def get_identifiers(self) -> list[str]:
        ids: list[str] = []
        text = self.template
        delim = self.delimiter
        if not delim:
            return ids
        delim_len = len(delim)
        idx = 0
        length = len(text)
        while idx < length:
            next_idx = text.find(delim, idx)
            if next_idx == -1 or next_idx == length - 1:
                break
            if next_idx + delim_len > length - 1:
                break
            next_char = text[next_idx + delim_len]
            if text.startswith(delim, next_idx + delim_len):
                idx = next_idx + delim_len * 2
                continue
            if next_char == "{":
                brace_start = next_idx + delim_len + 1
                brace_end = text.find("}", brace_start)
                if brace_end == -1:
                    break
                name = text[brace_start:brace_end]
                if name and _scan_identifier(name, 0) is not None and name not in ids:
                    ids.append(name)
                idx = brace_end + 1
                continue
            ident = _scan_identifier(text, next_idx + delim_len)
            if ident is None:
                idx = next_idx + delim_len
                continue
            name, end = ident
            if name not in ids:
                ids.append(name)
            idx = end
        return ids


def _formatter_field_name_split(
    field_name: str,
) -> tuple[object, list[tuple[bool, object]]]:
    if field_name == "":
        return "", []
    end = 0
    length = len(field_name)
    while end < length and field_name[end] not in ".[":
        end += 1
    first = field_name[:end]
    if first.isdigit():
        first_val: object = int(first)
    else:
        first_val = first
    rest: list[tuple[bool, object]] = []
    idx = end
    while idx < length:
        if field_name[idx] == ".":
            idx += 1
            start = idx
            while idx < length and field_name[idx] not in ".[":
                idx += 1
            rest.append((True, field_name[start:idx]))
            continue
        if field_name[idx] == "[":
            idx += 1
            start = idx
            while idx < length and field_name[idx] != "]":
                idx += 1
            if idx >= length:
                raise ValueError("expected ']' before end of string")
            key_text = field_name[start:idx]
            if key_text.isdigit():
                key: object = int(key_text)
            else:
                key = key_text
            rest.append((False, key))
            idx += 1
            continue
        break
    return first_val, rest


def _formatter_parser(format_string: str):
    length = len(format_string)
    idx = 0
    literal: list[str] = []
    while idx < length:
        ch = format_string[idx]
        if ch == "{":
            if idx + 1 < length and format_string[idx + 1] == "{":
                literal.append("{")
                idx += 2
                continue
            literal_text = "".join(literal)
            literal = []
            idx += 1
            if idx >= length:
                raise ValueError("Single '{' encountered in format string")
            field_name, format_spec, conversion, idx = _parse_field(format_string, idx)
            yield literal_text, field_name, format_spec, conversion
            continue
        if ch == "}":
            if idx + 1 < length and format_string[idx + 1] == "}":
                literal.append("}")
                idx += 2
                continue
            raise ValueError("Single '}' encountered in format string")
        literal.append(ch)
        idx += 1
    if literal or length == 0:
        yield "".join(literal), None, None, None


def _parse_field(format_string: str, idx: int) -> tuple[str, str, str | None, int]:
    length = len(format_string)
    field_start = idx
    bracket_depth = 0
    while idx < length:
        ch = format_string[idx]
        if ch == "[":
            bracket_depth += 1
            idx += 1
            continue
        if ch == "]" and bracket_depth:
            bracket_depth -= 1
            idx += 1
            continue
        if bracket_depth == 0 and ch in "!:}":
            break
        idx += 1
    if idx >= length:
        if idx == field_start:
            raise ValueError("Single '{' encountered in format string")
        raise ValueError("expected '}' before end of string")
    field_name = format_string[field_start:idx]
    conversion: str | None = None
    if format_string[idx] == "!":
        if idx + 1 >= length:
            raise ValueError("unmatched '{' in format spec")
        conversion = format_string[idx + 1]
        idx += 2
        if idx >= length:
            raise ValueError("unmatched '{' in format spec")
        if format_string[idx] not in ":}":
            raise ValueError("expected ':' after conversion specifier")
    format_spec = ""
    if format_string[idx] == ":":
        idx += 1
        spec_start = idx
        nested = 0
        while idx < length:
            ch = format_string[idx]
            if ch == "{":
                if idx + 1 < length and format_string[idx + 1] == "{":
                    idx += 2
                    continue
                nested += 1
                idx += 1
                continue
            if ch == "}":
                if idx + 1 < length and format_string[idx + 1] == "}":
                    idx += 2
                    continue
                if nested == 0:
                    break
                nested -= 1
                idx += 1
                continue
            idx += 1
        if idx >= length:
            raise ValueError("unmatched '{' in format spec")
        format_spec = format_string[spec_start:idx]
    if idx >= length or format_string[idx] != "}":
        raise ValueError("expected '}' before end of string")
    idx += 1
    return field_name, format_spec, conversion, idx


class Formatter:
    def format(self, format_string: str, /, *args, **kwargs) -> str:
        return self.vformat(format_string, args, kwargs)

    def vformat(
        self, format_string: str, args: tuple[object, ...], kwargs: dict
    ) -> str:
        used_args: set[object] = set()
        result, _ = self._vformat(format_string, args, kwargs, used_args, 2)
        self.check_unused_args(used_args, args, kwargs)
        return result

    def _vformat(
        self,
        format_string: str,
        args: tuple[object, ...],
        kwargs: dict,
        used_args: set[object],
        recursion_depth: int,
        auto_arg_index: int | bool = 0,
    ) -> tuple[str, int | bool]:
        if recursion_depth < 0:
            raise ValueError("Max string recursion exceeded")
        result: list[str] = []
        for literal_text, field_name, format_spec, conversion in self.parse(
            format_string
        ):
            if literal_text:
                result.append(literal_text)
            if field_name is not None:
                field_first, _ = _formatter_field_name_split(field_name)
                if field_first == "":
                    if auto_arg_index is False:
                        raise ValueError(
                            "cannot switch from manual field "
                            "specification to automatic field numbering"
                        )
                    field_name = f"{auto_arg_index}{field_name}"
                    auto_arg_index = int(auto_arg_index) + 1
                elif isinstance(field_first, int):
                    if auto_arg_index:
                        raise ValueError(
                            "cannot switch from automatic field "
                            "numbering to manual field specification"
                        )
                    auto_arg_index = False
                obj, arg_used = self.get_field(field_name, args, kwargs)
                used_args.add(arg_used)
                obj = self.convert_field(obj, conversion)
                format_spec, auto_arg_index = self._vformat(
                    format_spec,
                    args,
                    kwargs,
                    used_args,
                    recursion_depth - 1,
                    auto_arg_index=auto_arg_index,
                )
                result.append(self.format_field(obj, format_spec))
        return "".join(result), auto_arg_index

    def get_value(self, key: object, args: tuple[object, ...], kwargs: dict) -> object:
        if isinstance(key, int):
            return args[key]
        return kwargs[key]

    def check_unused_args(
        self, used_args: set[object], args: tuple, kwargs: dict
    ) -> None:
        return None

    def format_field(self, value: object, format_spec: str) -> str:
        return format(value, format_spec)

    def convert_field(self, value: object, conversion: str | None) -> object:
        if conversion is None:
            return value
        if conversion == "s":
            return str(value)
        if conversion == "r":
            return repr(value)
        if conversion == "a":
            return ascii(value)
        raise ValueError(f"Unknown conversion specifier {conversion!s}")

    def parse(self, format_string: str):
        return _formatter_parser(format_string)

    def get_field(
        self, field_name: str, args: tuple[object, ...], kwargs: dict
    ) -> tuple[object, object]:
        first, rest = _formatter_field_name_split(field_name)
        obj = self.get_value(first, args, kwargs)
        for is_attr, key in rest:
            if is_attr:
                obj = getattr(obj, cast(str, key))
            else:
                obj = cast(Any, obj)[key]
        return obj, first


def capwords(s: str, sep: str | None = None) -> str:
    if sep is None:
        parts: list[str] = []
        for part in s.split():
            parts.append(part.capitalize())
        return " ".join(parts)
    parts: list[str] = []
    for part in s.split(sep):
        parts.append(part.capitalize())
    return sep.join(parts)
