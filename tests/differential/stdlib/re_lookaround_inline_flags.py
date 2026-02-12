"""Purpose: differential coverage for re lookarounds and inline flags."""

import re

print("lookahead", bool(re.search(r"a(?=b)", "ab")))
print("lookbehind", bool(re.search(r"(?<=a)b", "ab")))

try:
    re.compile(r"(?<=ab)c")
    print("lookbehind_fixed", "ok")
except Exception as exc:
    print("lookbehind_fixed", type(exc).__name__)

try:
    re.compile(r"(?i)ab(?-i:CD)")
    print("inline", "ok")
except Exception as exc:
    print("inline", type(exc).__name__)
