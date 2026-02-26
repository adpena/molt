"""Minimal JSON shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


import operator
import re
from typing import Any, Callable, Iterable

_MOLT_JSON_PARSE_SCALAR = _require_intrinsic("molt_json_parse_scalar_obj", globals())
_MOLT_JSON_ENCODE_BASESTRING = _require_intrinsic(
    "molt_json_encode_basestring_obj", globals()
)
_MOLT_JSON_ENCODE_BASESTRING_ASCII = _require_intrinsic(
    "molt_json_encode_basestring_ascii_obj", globals()
)
_MOLT_JSON_DETECT_ENCODING = _require_intrinsic("molt_json_detect_encoding", globals())
_MOLT_JSON_LOADS_EX = _require_intrinsic("molt_json_loads_ex", globals())
_MOLT_JSON_DUMPS_EX = _require_intrinsic("molt_json_dumps_ex", globals())
_MOLT_JSON_RAW_DECODE_EX = _require_intrinsic("molt_json_raw_decode_ex", globals())


__all__ = [
    "dump",
    "dumps",
    "load",
    "loads",
    "JSONDecoder",
    "JSONDecodeError",
    "JSONEncoder",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): continue full json parity work (JSONDecodeError formatting nuances and remaining edge-case diagnostics).


class JSONDecodeError(ValueError):
    def __init__(self, msg: str, doc: str, pos: int) -> None:
        self.msg = msg
        self.doc = doc
        self.pos = int(pos)
        self.lineno, self.colno = _calc_lineno_col(doc, self.pos)
        super().__init__(self.__str__())

    def __reduce__(self):
        return self.__class__, (self.msg, self.doc, self.pos)

    def __str__(self) -> str:
        return f"{self.msg}: line {self.lineno} column {self.colno} (char {self.pos})"


def _calc_lineno_col(doc: str, pos: int) -> tuple[int, int]:
    lineno = doc.count("\n", 0, pos) + 1
    line_start = doc.rfind("\n", 0, pos)
    if line_start < 0:
        colno = pos + 1
    else:
        colno = pos - line_start
    return lineno, colno


def _decode_bytes_payload(payload: bytes | bytearray) -> str:
    data = bytes(payload)
    if not data:
        return ""
    encoding = _MOLT_JSON_DETECT_ENCODING(data)
    if encoding == "utf-8" and data.startswith(b"\xef\xbb\xbf"):
        return data.decode("utf-8-sig")
    return data.decode(encoding)


def _coerce_json_text(payload: str | bytes | bytearray) -> str:
    if isinstance(payload, str):
        return payload
    if isinstance(payload, (bytes, bytearray)):
        return _decode_bytes_payload(payload)
    raise TypeError(
        f"the JSON object must be str, bytes or bytearray, not {type(payload).__name__}"
    )


def _default_separators(indent: int | str | None) -> tuple[str, str]:
    return (", ", ": ") if indent is None else (",", ": ")


_JSON_ERROR_RE = re.compile(
    r"^(?P<msg>.*): line (?P<line>\d+) column (?P<col>\d+) \(char (?P<pos>\d+)\)$"
)


def _raise_json_decode_from_value_error(exc: ValueError, doc: str) -> None:
    text = str(exc)
    match = _JSON_ERROR_RE.match(text)
    if match is None:
        raise exc
    msg = match.group("msg")
    pos = int(match.group("pos"))
    raise JSONDecodeError(msg, doc, pos) from None


def _walk_circular_markers(obj: Any, markers: set[int]) -> None:
    if isinstance(obj, dict):
        marker = id(obj)
        if marker in markers:
            raise ValueError("Circular reference detected")
        markers.add(marker)
        for key, value in obj.items():
            _walk_circular_markers(key, markers)
            _walk_circular_markers(value, markers)
        markers.remove(marker)
        return
    if isinstance(obj, (list, tuple)):
        marker = id(obj)
        if marker in markers:
            raise ValueError("Circular reference detected")
        markers.add(marker)
        for item in obj:
            _walk_circular_markers(item, markers)
        markers.remove(marker)


def _validate_no_circular_references(obj: Any) -> None:
    _walk_circular_markers(obj, set())


def _try_intrinsic_dumps(
    obj: Any,
    *,
    skipkeys: bool,
    ensure_ascii: bool,
    check_circular: bool,
    allow_nan: bool,
    sort_keys: bool,
    indent: int | str | None,
    separators: tuple[str, str],
    default: Callable[[Any], Any] | None,
) -> str:
    return _MOLT_JSON_DUMPS_EX(
        obj,
        skipkeys,
        ensure_ascii,
        check_circular,
        allow_nan,
        sort_keys,
        indent,
        separators[0],
        separators[1],
        default,
    )


def loads(
    s: str | bytes | bytearray,
    *,
    cls: Any | None = None,
    object_hook: Callable[[dict[str, Any]], Any] | None = None,
    parse_float: Callable[[str], Any] | None = None,
    parse_int: Callable[[str], Any] | None = None,
    parse_constant: Callable[[str], Any] | None = None,
    object_pairs_hook: Callable[[list[tuple[str, Any]]], Any] | None = None,
    **kw: Any,
) -> Any:
    strict_explicit = "strict" in kw
    strict = kw.pop("strict", True)
    decoder_cls = JSONDecoder if cls is None else cls
    text = _coerce_json_text(s)
    if isinstance(s, str) and text.startswith("\ufeff"):
        raise JSONDecodeError("Unexpected UTF-8 BOM (decode using utf-8-sig)", text, 0)
    if decoder_cls is JSONDecoder and not kw:
        try:
            return _MOLT_JSON_LOADS_EX(
                text,
                parse_float,
                parse_int,
                parse_constant,
                object_hook,
                object_pairs_hook,
                strict,
            )
        except ValueError as exc:
            _raise_json_decode_from_value_error(exc, text)

    decoder_kwargs = dict(kw)
    if object_hook is not None:
        decoder_kwargs["object_hook"] = object_hook
    if parse_float is not None:
        decoder_kwargs["parse_float"] = parse_float
    if parse_int is not None:
        decoder_kwargs["parse_int"] = parse_int
    if parse_constant is not None:
        decoder_kwargs["parse_constant"] = parse_constant
    if object_pairs_hook is not None:
        decoder_kwargs["object_pairs_hook"] = object_pairs_hook
    if strict_explicit or strict is not True:
        decoder_kwargs["strict"] = strict

    decoder = decoder_cls(**decoder_kwargs)
    return decoder.decode(text)


def load(
    fp: Any,
    *,
    cls: Any | None = None,
    object_hook: Callable[[dict[str, Any]], Any] | None = None,
    parse_float: Callable[[str], Any] | None = None,
    parse_int: Callable[[str], Any] | None = None,
    parse_constant: Callable[[str], Any] | None = None,
    object_pairs_hook: Callable[[list[tuple[str, Any]]], Any] | None = None,
    **kw: Any,
) -> Any:
    return loads(
        fp.read(),
        cls=cls,
        object_hook=object_hook,
        parse_float=parse_float,
        parse_int=parse_int,
        parse_constant=parse_constant,
        object_pairs_hook=object_pairs_hook,
        **kw,
    )


def dumps(
    obj: Any,
    *,
    cls: Any | None = None,
    skipkeys: bool = False,
    ensure_ascii: bool = True,
    check_circular: bool = True,
    allow_nan: bool = True,
    sort_keys: bool = False,
    indent: int | str | None = None,
    separators: tuple[str, str] | None = None,
    default: Callable[[Any], Any] | None = None,
    **kw: Any,
) -> str:
    if separators is None:
        separators = _default_separators(indent)
    elif len(separators) != 2:
        raise ValueError("separators must be a (item, key) tuple")
    encoder_cls = cls or JSONEncoder
    if encoder_cls is JSONEncoder and not kw:
        return _try_intrinsic_dumps(
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
    encoder = encoder_cls(
        skipkeys=skipkeys,
        ensure_ascii=ensure_ascii,
        check_circular=check_circular,
        allow_nan=allow_nan,
        sort_keys=sort_keys,
        indent=indent,
        separators=separators,
        default=default,
        **kw,
    )
    return encoder.encode(obj)


def dump(
    obj: Any,
    fp: Any,
    *,
    cls: Any | None = None,
    skipkeys: bool = False,
    ensure_ascii: bool = True,
    check_circular: bool = True,
    allow_nan: bool = True,
    sort_keys: bool = False,
    indent: int | str | None = None,
    separators: tuple[str, str] | None = None,
    default: Callable[[Any], Any] | None = None,
    **kw: Any,
) -> None:
    text = dumps(
        obj,
        cls=cls,
        skipkeys=skipkeys,
        ensure_ascii=ensure_ascii,
        check_circular=check_circular,
        allow_nan=allow_nan,
        sort_keys=sort_keys,
        indent=indent,
        separators=separators,
        default=default,
        **kw,
    )
    fp.write(text)


class JSONEncoder:
    def __init__(
        self,
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
        if separators is None:
            separators = (", ", ": ") if indent is None else (",", ": ")
        elif len(separators) != 2:
            raise ValueError("separators must be a (item, key) tuple")
        self.skipkeys = skipkeys
        self.ensure_ascii = ensure_ascii
        self.check_circular = check_circular
        self.allow_nan = allow_nan
        self.sort_keys = sort_keys
        self.indent = indent
        self.separators = separators
        self._default = default

    def default(self, obj: Any) -> Any:
        if self._default is not None:
            return self._default(obj)
        raise TypeError(f"Object of type {type(obj).__name__} is not JSON serializable")

    def encode(self, obj: Any) -> str:
        default_cb: Callable[[Any], Any] | None = self._default
        if type(self) is not JSONEncoder:
            default_cb = self.default
        return _try_intrinsic_dumps(
            obj,
            skipkeys=self.skipkeys,
            ensure_ascii=self.ensure_ascii,
            check_circular=self.check_circular,
            allow_nan=self.allow_nan,
            sort_keys=self.sort_keys,
            indent=self.indent,
            separators=self.separators,
            default=default_cb,
        )

    def iterencode(self, obj: Any) -> Iterable[str]:
        yield self.encode(obj)


class JSONDecoder:
    def __init__(
        self,
        *,
        object_hook: Callable[[dict[str, Any]], Any] | None = None,
        parse_float: Callable[[str], Any] | None = None,
        parse_int: Callable[[str], Any] | None = None,
        parse_constant: Callable[[str], Any] | None = None,
        object_pairs_hook: Callable[[list[tuple[str, Any]]], Any] | None = None,
        strict: bool = True,
    ) -> None:
        self.object_hook = object_hook
        self.parse_float = parse_float
        self.parse_int = parse_int
        self.parse_constant = parse_constant
        self.object_pairs_hook = object_pairs_hook
        self.strict = strict

    def decode(self, s: str) -> Any:
        if not isinstance(s, str):
            raise TypeError(
                f"the JSON object must be str, bytes or bytearray, not {type(s).__name__}"
            )
        try:
            return _MOLT_JSON_LOADS_EX(
                s,
                self.parse_float,
                self.parse_int,
                self.parse_constant,
                self.object_hook,
                self.object_pairs_hook,
                self.strict,
            )
        except ValueError as exc:
            _raise_json_decode_from_value_error(exc, s)

    def raw_decode(self, s: str, idx: int = 0) -> tuple[Any, int]:
        if not isinstance(s, str):
            raise TypeError(f"first argument must be a string, not {type(s).__name__}")
        idx = operator.index(idx)
        if idx < 0:
            raise ValueError("idx cannot be negative")
        try:
            return _MOLT_JSON_RAW_DECODE_EX(
                s,
                idx,
                self.parse_float,
                self.parse_int,
                self.parse_constant,
                self.object_hook,
                self.object_pairs_hook,
                self.strict,
            )
        except ValueError as exc:
            _raise_json_decode_from_value_error(exc, s)
