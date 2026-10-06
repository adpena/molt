"""Purpose: differential coverage for collections.Counter basics."""

from collections import Counter

c = Counter("ababc")
print(c["a"], c["b"], c["c"])
print(c.most_common(2))
print(c.total())

c.update("aa")
print(c["a"])


# Temporary wrappers own their registry storage for the whole method call.
print("temporary", Counter(["a", "b", "a"])["a"], len(Counter(("a", "b", "a"))))


class CustomCounter(Counter):
    def __init__(self, items):
        super().__init__(items)
        self["constructed"] = 7

    def __getitem__(self, key):
        return super().__getitem__(key) + 100


print("subclass", CustomCounter(["a", "a"])["a"], CustomCounter([])["constructed"])


def live_counter_binding():
    import collections

    original = collections.Counter
    try:
        collections.Counter = CustomCounter
        return collections.Counter(["a", "a"])["a"]
    finally:
        collections.Counter = original


print("rebound", live_counter_binding())
