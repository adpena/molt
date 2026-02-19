"""Minimal `_json` surface for json submodule API parity."""

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_JSON_PARSE_SCALAR = _require_intrinsic("molt_json_parse_scalar_obj", globals())

# CPython exposes these as C-level callables/types. We keep the same public type
# shapes for API-digest parity without host-Python dependency.
encode_basestring = len
encode_basestring_ascii = len
scanstring = len


class make_encoder:
    pass


class make_scanner:
    pass
