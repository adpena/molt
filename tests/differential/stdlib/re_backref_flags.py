"""Purpose: differential coverage for re backrefs and inline flags."""

import re

pattern = re.compile(r"(?i)(ab)(cd)\\1")
print("match", bool(pattern.search("ABCDab")))

try:
    re.compile(r"(?i:ab)(?i:cd)")
    print("inline", "ok")
except Exception as exc:
    print("inline", type(exc).__name__)
