"""Purpose: differential coverage for re compile flags errors."""

import re


print(bool(re.compile(r"a", re.IGNORECASE | re.MULTILINE)))

try:
    re.compile(r"a", 1 << 30)
    print("ok")
except Exception as exc:
    print(type(exc).__name__)

try:
    re.compile(r"(")
except Exception as exc:
    print(type(exc).__name__)
