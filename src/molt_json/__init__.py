"""CPython fallback for the Molt JSON package.

The real Molt package is implemented in Rust/WASM. This shim keeps tests and
local tooling working in CPython environments.
"""

from __future__ import annotations

import json
import re
from typing import Any

from molt import shims

_INT_RE = re.compile(r"-?(0|[1-9]\\d*)\\Z")


def _parse_int_runtime(data: str) -> int:
    lib = shims.load_runtime()
    if lib is None:
        raise RuntimeError("Molt runtime library not available")
    buf = data.encode("utf-8")
    return int(lib.molt_json_parse_int(buf, len(buf)))


def parse(data: str) -> Any:
    trimmed = data.strip()
    lib = shims.load_runtime()
    if lib is not None and _INT_RE.fullmatch(trimmed):
        return _parse_int_runtime(trimmed)
    return json.loads(data)
