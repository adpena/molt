"""Purpose: differential coverage for re lookaround backref."""

import re


text = "ababa"
print(re.findall(r"(?=(aba))", text))
print(re.search(r"(ab)\1", "abab").group(0))

m = re.search(r"(?<=a)b", "ab")
print(m.group(0))

try:
    re.compile(r"(?<=ab)c")
    print("ok")
except Exception as exc:
    print(type(exc).__name__)
