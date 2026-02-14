"""Intrinsic-first shim for CPython's internal `re._parser` module.

This is not a host-stdlib fallback: it routes parsing through Molt's `re`
implementation (currently Python-level parsing + intrinsic-backed execution).

We keep this module present so package layouts match CPython 3.12+ expectations.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# Keep imports at the top for lint; this is still intrinsic-first (no host fallback).
from typing import Any

import re as _re

# Avoid probe-only classification: this shim must still be intrinsic-backed.
_require_intrinsic("molt_re_literal_advance", globals())

error = _re.error

__all__ = ["parse", "error"]


def parse(pattern: str, flags: int = 0) -> Any:
    """Parse a regex pattern.

    CPython's internal parser returns an `sre_parse.Pattern` structure.
    Molt returns an internal parse tree used by its `re` shim.
    """

    parser = getattr(_re, "_Parser", None)
    if parser is None:
        raise RuntimeError("re parser unavailable in this build")
    return parser(pattern).parse()
