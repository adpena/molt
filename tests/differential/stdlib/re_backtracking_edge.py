"""Purpose: differential coverage for re backtracking edge."""

import re


pattern = re.compile(r"(a+)+$")
print(bool(pattern.match("a" * 10)))
print(bool(pattern.match("a" * 10 + "!")))
