"""Minimal JSON shim for Molt — fully intrinsic-backed."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_JSON_PARSE_SCALAR = _require_intrinsic("molt_json_parse_scalar_obj")
_MOLT_JSON_ENCODE_BASESTRING = _require_intrinsic("molt_json_encode_basestring_obj")
_MOLT_JSON_ENCODE_BASESTRING_ASCII = _require_intrinsic(
    "molt_json_encode_basestring_ascii_obj"
)
_MOLT_JSON_DETECT_ENCODING = _require_intrinsic("molt_json_detect_encoding")
_MOLT_JSON_LOADS_EX = _require_intrinsic("molt_json_loads_ex")
_MOLT_JSON_DUMPS_EX = _require_intrinsic("molt_json_dumps_ex")
_MOLT_JSON_RAW_DECODE_EX = _require_intrinsic("molt_json_raw_decode_ex")
_MOLT_JSON_CALC_LINENO_COL = _require_intrinsic("molt_json_calc_lineno_col")
_MOLT_JSON_COERCE_TEXT = _require_intrinsic("molt_json_coerce_text")
_MOLT_JSON_DEFAULT_SEPARATORS = _require_intrinsic("molt_json_default_separators")
_MOLT_JSON_FORMAT_DECODE_ERROR = _require_intrinsic("molt_json_format_decode_error")
_MOLT_JSON_PARSE_ERROR_MSG = _require_intrinsic("molt_json_parse_error_msg")


__all__ = [
    "dump",
    "dumps",
    "load",
    "loads",
    "JSONDecoder",
    "JSONDecodeError",
    "JSONEncoder",
]


class JSONDecodeError(ValueError):
    def __init__(self, msg: str, doc: str, pos: int) -> None:
        self.msg = msg
        self.doc = doc
        self.pos = int(pos)
        self.lineno, self.colno = _MOLT_JSON_CALC_LINENO_COL(doc, self.pos)
        super().__init__(self.__str__())

    def __reduce__(self):
        return self.__class__, (self.msg, self.doc, self.pos)

    def __str__(self) -> str:
        return str(_MOLT_JSON_FORMAT_DECODE_ERROR(self.msg, self.doc, self.pos))


def _raise_json_decode_from_value_error(exc: ValueError, doc: str) -> None:
    result = _MOLT_JSON_PARSE_ERROR_MSG(str(exc))
    if result is None:
        raise exc
    msg, pos = result
    raise JSONDecodeError(msg, doc, pos) from None


def _try_intrinsic_dumps(
    obj,
    *,
    skipkeys,
    ensure_ascii,
    check_circular,
    allow_nan,
    sort_keys,
    indent,
    separators,
    default,
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
    s,
    *,
    cls=None,
    object_hook=None,
    parse_float=None,
    parse_int=None,
    parse_constant=None,
    object_pairs_hook=None,
    **kw,
):
    strict_explicit = "strict" in kw
    strict = kw.pop("strict", True)
    if cls is not None:
        decoder_cls = cls
    else:
        decoder_cls = JSONDecoder
    text = _MOLT_JSON_COERCE_TEXT(s)
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
    fp,
    *,
    cls=None,
    object_hook=None,
    parse_float=None,
    parse_int=None,
    parse_constant=None,
    object_pairs_hook=None,
    **kw,
):
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
    obj,
    *,
    cls=None,
    skipkeys=False,
    ensure_ascii=True,
    check_circular=True,
    allow_nan=True,
    sort_keys=False,
    indent=None,
    separators=None,
    default=None,
    **kw,
) -> str:
    if separators is None:
        separators = _MOLT_JSON_DEFAULT_SEPARATORS(indent)
    elif len(separators) != 2:
        raise ValueError("separators must be a (item, key) tuple")
    if cls is not None:
        encoder_cls = cls
    else:
        encoder_cls = JSONEncoder
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
    obj,
    fp,
    *,
    cls=None,
    skipkeys=False,
    ensure_ascii=True,
    check_circular=True,
    allow_nan=True,
    sort_keys=False,
    indent=None,
    separators=None,
    default=None,
    **kw,
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
        skipkeys=False,
        ensure_ascii=True,
        check_circular=True,
        allow_nan=True,
        sort_keys=False,
        indent=None,
        separators=None,
        default=None,
    ) -> None:
        if separators is None:
            separators = _MOLT_JSON_DEFAULT_SEPARATORS(indent)
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

    def default(self, obj):
        if self._default is not None:
            return self._default(obj)
        raise TypeError(f"Object of type {type(obj).__name__} is not JSON serializable")

    def encode(self, obj) -> str:
        default_cb = self._default
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

    def iterencode(self, obj):
        yield self.encode(obj)


class JSONDecoder:
    def __init__(
        self,
        *,
        object_hook=None,
        parse_float=None,
        parse_int=None,
        parse_constant=None,
        object_pairs_hook=None,
        strict=True,
    ) -> None:
        self.object_hook = object_hook
        self.parse_float = parse_float
        self.parse_int = parse_int
        self.parse_constant = parse_constant
        self.object_pairs_hook = object_pairs_hook
        self.strict = strict

    def decode(self, s: str):
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

    def raw_decode(self, s: str, idx: int = 0):
        if not isinstance(s, str):
            raise TypeError(f"first argument must be a string, not {type(s).__name__}")
        import operator

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


globals().pop("_require_intrinsic", None)
