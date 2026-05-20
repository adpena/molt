"""Purpose: Counter preserves string-key semantics with repeated distinct strings."""

from collections import Counter


words = ("alpha beta alpha gamma beta alpha " * 3).split()
c = Counter(words)
print(c["alpha"], c["beta"], c["gamma"], len(c))

c["beta"] = 10
copy = c.copy()
del c["alpha"]
print(c["alpha"], c["beta"], len(c), copy["alpha"], copy["beta"], len(copy))

c.update(["beta", "delta", "delta", "alpha"])
print(c["alpha"], c["beta"], c["delta"], sorted(c.items()))
