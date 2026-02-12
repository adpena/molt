"""Trigger the PEP 3120 syntax error expected by CPython tests."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

if False:
    _require_intrinsic("molt_capabilities_has", globals())

raise SyntaxError(
    "utf-8 codec can't decode byte",
    ("badsyntax_pep3120.py", 1, 1, 'print("b\\ufffdse")'),
)
