"""Trigger the PEP 3120 syntax error expected by CPython tests."""

from __future__ import annotations

raise SyntaxError(
    "utf-8 codec can't decode byte",
    ("badsyntax_pep3120.py", 1, 1, 'print("b\\ufffdse")'),
)
