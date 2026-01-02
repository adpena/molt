"""CPython fallback for the Molt JSON package.

The real Molt package is implemented in Rust/WASM. This shim keeps tests and
local tooling working in CPython environments.
"""

from __future__ import annotations

import json
from typing import Any


def parse(data: str) -> Any:
    return json.loads(data)
