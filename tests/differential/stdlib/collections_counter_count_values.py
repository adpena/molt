"""Purpose: Counter(iterable) counting, at every boundary of the in-place count.

The count loop may add one to a found count in place only when the count and
its successor are inline ints and nothing ran since the probe. Every other
value or mapping must take the statement `m[k] = m.get(k, 0) + 1` in order.
"""

from collections import Counter

# Counts that cross any inline-int width: the successor leaves the inline range
# for some k, and the result must stay exact and an int.
for k in range(40, 66):
    start = 2**k - 1
    c = Counter({"w": start})
    c.update(["w", "w"])
    print("width", k, c["w"] - start, type(c["w"]).__name__)
    c = Counter({"w": -(2**k)})
    c.update(["w"])
    print("negative-width", k, c["w"] + 2**k, type(c["w"]).__name__)

c = Counter({"a": -1})
c.update(["a", "a"])
print("negative", c["a"])

c = Counter({"a": 1.5})
c.update(["a"])
print("float", c["a"], type(c["a"]).__name__)

c = Counter({"a": True})
c.update(["a"])
print("bool", c["a"], type(c["a"]).__name__)

c = Counter({"a": False})
c.update(["a", "b", "a"])
print("bool-false", sorted(c.items()), type(c["a"]).__name__)


class Grows:
    """A count whose addition inserts keys, forcing the table to grow."""

    def __init__(self, counter, value):
        self.counter = counter
        self.value = value

    def __add__(self, other):
        for i in range(32):
            self.counter[f"grown{i}"] = i
        return self.value + other


c = Counter()
c["x"] = Grows(c, 10)
c.update(["x", "y", "x"])
print("grows", c["x"], c["y"], len(c))


class Recorded(Counter):
    def __setitem__(self, key, value):
        print("setitem", key, value)
        super().__setitem__(key, value)


r = Recorded()
r.update(["p", "q", "p"])
print("recorded", sorted(r.items()))


class MutatingKey:
    """Equal to the string it wraps; comparing grows the counter first."""

    def __init__(self, counter, text):
        self.counter = counter
        self.text = text

    def __hash__(self):
        return hash(self.text)

    def __eq__(self, other):
        if isinstance(other, str):
            for i in range(32):
                self.counter[i] = i
            return other == self.text
        return NotImplemented


c = Counter({"k": 5})
c.update([MutatingKey(c, "k"), "k"])
print("mutating-key", c["k"], len(c))

class CountedEq:
    """Equal, not identical, keys: each lookup of the stored key calls __eq__,
    so the count exposes how many lookups one element takes."""

    calls = 0

    def __init__(self, text):
        self.text = text

    def __hash__(self):
        return hash(self.text)

    def __eq__(self, other):
        CountedEq.calls += 1
        return isinstance(other, CountedEq) and other.text == self.text


c = Counter([CountedEq("e")])
CountedEq.calls = 0
c.update([CountedEq("e"), CountedEq("e")])
print("eq-calls", CountedEq.calls, list(c.values()))

words = "the quick the lazy the end quick".split()
c = Counter(words)
c.update(words)
print("words", sorted(c.items()))
