"""Purpose: differential coverage for collections.Counter basics."""

from collections import Counter

c = Counter("ababc")
print(c["a"], c["b"], c["c"])
print(c.most_common(2))
print(c.total())

c.update("aa")
print(c["a"])
