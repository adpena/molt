"""Minimal `json.decoder` compatibility surface."""

import re

from _intrinsics import require_intrinsic as _require_intrinsic
import json.scanner as scanner  # noqa: F401
from _json import scanstring as c_scanstring
from json import JSONDecodeError  # noqa: F401
from json import JSONDecoder  # noqa: F401

_MOLT_JSON_PARSE_SCALAR = _require_intrinsic("molt_json_parse_scalar_obj")

BACKSLASH = {
    '"': '"',
    "\\": "\\",
    "/": "/",
    "b": "\b",
    "f": "\f",
    "n": "\n",
    "r": "\r",
    "t": "\t",
}


# Keep CPython surface shape: `FLAGS` reports type name `RegexFlag`.
class _RegexFlag:
    __slots__ = ("_value",)

    def __init__(self, value):
        self._value = int(value)

    def __int__(self):
        return self._value

    def __index__(self):
        return self._value

    def __and__(self, other):
        return self._value & int(other)

    def __rand__(self, other):
        return int(other) & self._value

    def __or__(self, other):
        return self._value | int(other)

    def __ror__(self, other):
        return int(other) | self._value


_RegexFlag.__name__ = "RegexFlag"
FLAGS = _RegexFlag(re.VERBOSE | re.MULTILINE | re.DOTALL)
HEXDIGITS = re.compile(r"[0-9a-fA-F]{4}")
NaN = float("nan")
NegInf = float("-inf")
PosInf = float("inf")
STRINGCHUNK = re.compile(r'(.*?)(["\\\x00-\x1f])', FLAGS)
WHITESPACE = re.compile(r"[ \t\n\r]*", FLAGS)
WHITESPACE_STR = " \t\n\r"
scanstring = c_scanstring


def JSONArray(_s_and_end, _scan_once, _memo, _ws, _ws_end):
    raise NotImplementedError("JSONArray runtime path is not implemented")


def JSONObject(
    _s_and_end, _strict, _scan_once, _object_hook, _object_pairs_hook, _memo
):
    raise NotImplementedError("JSONObject runtime path is not implemented")


def py_scanstring(_s, _end, _strict=True):
    raise NotImplementedError("py_scanstring runtime path is not implemented")


globals().pop("_require_intrinsic", None)
