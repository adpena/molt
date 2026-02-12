"""Purpose: differential coverage for collections.defaultdict basics."""

from collections import defaultdict

m = defaultdict(int)
print(m["missing"])
m["a"] += 2
print(dict(m))
