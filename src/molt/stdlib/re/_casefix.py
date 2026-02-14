"""Intrinsic-first shim for CPython's internal `re._casefix` module.

CPython uses `_casefix` data tables for some Unicode case-insensitive handling.
Molt's regex engine routes matching into Rust intrinsics, and it may not require
these tables yet. We still provide the module to match CPython's package layout.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# Avoid probe-only classification: this shim must still be intrinsic-backed.
_require_intrinsic("molt_re_literal_advance", globals())

# Placeholder data surface: keep the name present without silently falling back
# to host Python tables.
EXTRA_CASES: dict[int, str] = {}

__all__ = ["EXTRA_CASES"]
