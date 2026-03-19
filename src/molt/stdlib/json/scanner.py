"""Minimal `json.scanner` compatibility surface."""

import re

from _intrinsics import require_intrinsic as _require_intrinsic
from _json import make_scanner as c_make_scanner

_MOLT_JSON_PARSE_SCALAR = _require_intrinsic("molt_json_parse_scalar_obj")

NUMBER_RE = re.compile(r"-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?")
make_scanner = c_make_scanner


def py_make_scanner(_context):
    def _scan_once(_s, _idx):
        raise NotImplementedError("py_make_scanner runtime path is not implemented")

    return _scan_once

globals().pop("_require_intrinsic", None)
