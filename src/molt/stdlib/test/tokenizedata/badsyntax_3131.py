"""Trigger the PEP 3131 syntax error expected by CPython tests."""

from __future__ import annotations

_CHAR = "\u20ac"
_MESSAGE = f"invalid character '{_CHAR}' (U+20AC)"

raise SyntaxError(
    _MESSAGE,
    ("badsyntax_3131.py", 2, 1, f"{_CHAR} = 2"),
)
