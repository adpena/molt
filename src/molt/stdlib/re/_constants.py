"""Intrinsic-first shim for CPython's internal `re._constants` module.

Molt keeps flag constants aligned with CPython's `re` module where applicable.
Unsupported internal details are intentionally not emulated.
"""

from __future__ import annotations


import re as _re

# Avoid probe-only classification: this shim must still be intrinsic-backed.

ASCII = _re.ASCII
DOTALL = _re.DOTALL
IGNORECASE = _re.IGNORECASE
LOCALE = _re.LOCALE
MULTILINE = _re.MULTILINE
UNICODE = _re.UNICODE
VERBOSE = _re.VERBOSE

A = _re.A
I = _re.I  # noqa: E741
L = _re.L
M = _re.M
S = _re.S
U = _re.U
X = _re.X

__all__ = [
    "ASCII",
    "DOTALL",
    "IGNORECASE",
    "LOCALE",
    "MULTILINE",
    "UNICODE",
    "VERBOSE",
    "A",
    "I",
    "L",
    "M",
    "S",
    "U",
    "X",
]
