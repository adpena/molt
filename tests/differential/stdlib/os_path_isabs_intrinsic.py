"""Purpose: intrinsic-backed os.path.isabs host-parity semantics."""

from __future__ import annotations

import os


CASES = [
    "",
    ".",
    "relative/path",
    "/absolute/path",
    "C:\\Windows\\System32",
    "C:relative\\segment",
    "\\\\server\\share\\file.txt",
    "\\rooted\\segment",
]


for value in CASES:
    print(value, os.path.isabs(value))
