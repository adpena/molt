"""Purpose: differential coverage for re conditional groups."""

import re

pattern = re.compile(r"(a)?b(?(1)c|d)")
print("match1", bool(pattern.fullmatch("abc")))
print("match2", bool(pattern.fullmatch("bd")))
