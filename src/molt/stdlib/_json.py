"""Intrinsic-backed compatibility surface for CPython's `_json`."""

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_JSON_DUMPS_EX = _require_intrinsic("molt_json_dumps_ex")
_MOLT_JSON_DEFAULT_SEPARATORS = _require_intrinsic("molt_json_default_separators")
_MOLT_JSON_PARSE_SCALAR = _require_intrinsic("molt_json_parse_scalar_obj")
encode_basestring = _require_intrinsic("molt_json_encode_basestring_obj")
encode_basestring_ascii = _require_intrinsic("molt_json_encode_basestring_ascii_obj")
scanstring = _require_intrinsic("molt_json_scanstring_obj")


class make_encoder:
    def __init__(
        self,
        markers,
        default,
        encoder,
        indent,
        key_separator,
        item_separator,
        sort_keys,
        skipkeys,
        allow_nan,
    ) -> None:
        self.markers = markers
        self.default = default
        self.encoder = encoder
        self.indent = indent
        self.key_separator = key_separator
        self.item_separator = item_separator
        self.sort_keys = sort_keys
        self.skipkeys = skipkeys
        self.allow_nan = allow_nan

    def __call__(self, obj, _current_indent_level):
        separators = (self.item_separator, self.key_separator)
        if separators == (None, None):
            separators = _MOLT_JSON_DEFAULT_SEPARATORS(self.indent)
        text = _MOLT_JSON_DUMPS_EX(
            obj,
            self.skipkeys,
            self.encoder is encode_basestring_ascii,
            self.markers is not None,
            self.allow_nan,
            self.sort_keys,
            self.indent,
            separators[0],
            separators[1],
            self.default,
        )
        return (text,)


class make_scanner:
    def __init__(self, context) -> None:
        self.context = context

    def __call__(self, string, idx):
        return self.context.raw_decode(string, idx)
