"""Minimal `_json` surface for json submodule API parity."""

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_JSON_PARSE_SCALAR = _require_intrinsic("molt_json_parse_scalar_obj", globals())
encode_basestring = _require_intrinsic("molt_json_encode_basestring_obj", globals())
encode_basestring_ascii = _require_intrinsic(
    "molt_json_encode_basestring_ascii_obj", globals()
)
scanstring = _require_intrinsic("molt_json_scanstring_obj", globals())

# CPython exposes these as C-level callables/types. `make_*` are classes in
# CPython's `_json` implementation; keep matching callable type shape here.


class make_encoder:
    pass


class make_scanner:
    pass
