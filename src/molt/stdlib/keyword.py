"""Keyword helpers for Molt (Python 3.12+)."""

from __future__ import annotations

try:
    from typing import TYPE_CHECKING
except Exception:
    TYPE_CHECKING = False

if TYPE_CHECKING:
    from typing import Any
else:
    Any = object()

__all__ = ["kwlist", "softkwlist", "iskeyword", "issoftkeyword"]

kwlist = [
    "False",
    "None",
    "True",
    "and",
    "as",
    "assert",
    "async",
    "await",
    "break",
    "class",
    "continue",
    "def",
    "del",
    "elif",
    "else",
    "except",
    "finally",
    "for",
    "from",
    "global",
    "if",
    "import",
    "in",
    "is",
    "lambda",
    "nonlocal",
    "not",
    "or",
    "pass",
    "raise",
    "return",
    "try",
    "while",
    "with",
    "yield",
]

softkwlist = ["_", "case", "match", "type"]


def iskeyword(value: Any) -> bool:
    if not isinstance(value, str):
        return False
    return value in kwlist


def issoftkeyword(value: Any) -> bool:
    if not isinstance(value, str):
        return False
    return value in softkwlist
