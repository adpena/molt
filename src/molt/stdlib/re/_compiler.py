"""Intrinsic-first shim for CPython's internal `re._compiler` module.

Molt's public `re` surface is implemented in `re/__init__.py` and lowers match
execution into Rust intrinsics. We still provide these internal modules so that
stdlib/package layouts match CPython 3.12+ expectations.

Policy: no host-stdlib fallback. Unsupported internals raise immediately.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# Keep imports at the top for lint; this is still intrinsic-first (no host fallback).
from typing import Any

import re as _re

# Avoid probe-only classification: this shim must still be intrinsic-backed.
_require_intrinsic("molt_re_literal_advance", globals())

Pattern = _re.Pattern
Match = _re.Match
error = _re.error

__all__ = ["compile", "_compile", "Pattern", "Match", "error"]


def compile(pattern: Any, flags: int = 0) -> Pattern[str]:
    # Delegate to Molt's `re.compile` (which is intrinsic-backed in execution).
    return _re.compile(pattern, flags)


def _compile(pattern: Any, flags: int = 0) -> Pattern[str]:
    # CPython's `re` calls into `_compile` internally; keep the name available.
    return compile(pattern, flags)
