"""Molt JSON parsing via runtime intrinsics."""

from __future__ import annotations

import json
from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

try:
    _MOLT_JSON_PARSE_SCALAR_OBJ = _require_intrinsic(
        "molt_json_parse_scalar_obj", globals()
    )
except RuntimeError:
    _MOLT_JSON_PARSE_SCALAR_OBJ = None


def parse(data: str) -> Any:
    if _MOLT_JSON_PARSE_SCALAR_OBJ is not None:
        return _MOLT_JSON_PARSE_SCALAR_OBJ(data)
    # Tooling-only CPython baseline path; compiled Molt binaries always use intrinsics.
    return json.loads(data)


__all__ = ["parse"]
