"""String constants and helpers for Molt."""

from __future__ import annotations

from typing import Any, NoReturn, cast

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_STRING_CAPITALIZE = _require_intrinsic("molt_string_capitalize")
_molt_template_scan = _require_intrinsic("molt_string_template_scan")
_molt_template_is_valid = _require_intrinsic("molt_string_template_is_valid")
_molt_template_get_identifiers = _require_intrinsic(
    "molt_string_template_get_identifiers"
)
_molt_formatter_parse = _require_intrinsic("molt_string_formatter_parse")
_molt_field_name_split = _require_intrinsic("molt_string_formatter_field_name_split")


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
        segments = _molt_template_scan(self.template, self.delimiter)
        out: list[str] = []
        for literal, var_name, original in segments:
            if literal:
                out.append(literal)
            if var_name is None:
                continue
            if safe:
                try:
                    out.append(str(mapping[var_name]))  # type: ignore[index]
                except KeyError:
                    out.append(original)  # type: ignore[arg-type]
            else:
                try:
                    out.append(str(mapping[var_name]))  # type: ignore[index]
                except KeyError:
                    self._invalid(self.template.find(original))  # type: ignore[arg-type]
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
        return bool(_molt_template_is_valid(self.template, self.delimiter))

    def get_identifiers(self) -> list[str]:
        return list(_molt_template_get_identifiers(self.template, self.delimiter))


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
                field_first, _ = _molt_field_name_split(field_name)
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
        return _molt_formatter_parse(format_string)

    def get_field(
        self, field_name: str, args: tuple[object, ...], kwargs: dict
    ) -> tuple[object, object]:
        first, rest = _molt_field_name_split(field_name)
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
            parts.append(_MOLT_STRING_CAPITALIZE(part))
        return " ".join(parts)
    parts: list[str] = []
    for part in s.split(sep):
        parts.append(_MOLT_STRING_CAPITALIZE(part))
    return sep.join(parts)


globals().pop("_require_intrinsic", None)
