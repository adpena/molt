"""Purpose: differential coverage for variable-length lookbehind errors."""

import re

try:
    re.compile(r"(?<=a+)b")
    print("ok", "unexpected")
except Exception as exc:
    print("err", type(exc).__name__)
