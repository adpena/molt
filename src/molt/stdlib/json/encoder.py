"""Minimal `json.encoder` compatibility surface."""

import re

from _intrinsics import require_intrinsic as _require_intrinsic
from _json import encode_basestring
from _json import encode_basestring as c_encode_basestring
from _json import encode_basestring_ascii
from _json import encode_basestring_ascii as c_encode_basestring_ascii
from _json import make_encoder as c_make_encoder
from json import JSONEncoder

_MOLT_JSON_PARSE_SCALAR = _require_intrinsic("molt_json_parse_scalar_obj", globals())

ESCAPE = re.compile(r'[\x00-\x1f\\"\b\f\n\r\t]')
ESCAPE_ASCII = re.compile(r'([\\\\"]|[^\ -~])')
ESCAPE_DCT = {
    "\\": "\\\\",
    '"': '\\"',
    "\b": "\\b",
    "\f": "\\f",
    "\n": "\\n",
    "\r": "\\r",
    "\t": "\\t",
}
HAS_UTF8 = re.compile(r"[\x80-\xff]")
INFINITY = float("inf")


def py_encode_basestring(value):
    return '"' + str(value).replace("\\", "\\\\").replace('"', '\\"') + '"'


def py_encode_basestring_ascii(value):
    return py_encode_basestring(value)
