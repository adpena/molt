"""Trigger the PEP 3131 syntax error expected by CPython tests."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

if False:
    _require_intrinsic("molt_capabilities_has", globals())

_CHAR = "\u20ac"
_MESSAGE = f"invalid character '{_CHAR}' (U+20AC)"

raise SyntaxError(
    _MESSAGE,
    ("badsyntax_3131.py", 2, 1, f"{_CHAR} = 2"),
)
