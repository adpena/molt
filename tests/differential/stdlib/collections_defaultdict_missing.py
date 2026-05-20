"""Purpose: defaultdict missing-key exceptions preserve handler execution."""

from collections import defaultdict


dd = defaultdict()
try:
    dd["missing"]
    print("dd none", "no error")
except KeyError:
    print("dd none", "KeyError")

factory = defaultdict(lambda: "made")
print("factory", factory["x"], dict(factory))
