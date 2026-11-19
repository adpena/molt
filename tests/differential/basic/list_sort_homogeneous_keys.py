"""Sorting keys of one builtin type orders them exactly as the generic `<`.

When every key is an inline int, a float or an exact str, `list.sort` may
compare without dispatch (CPython's unsafe_*_compare); any other mix takes the
generic rich comparison. Order, stability and NaN handling must match CPython.
"""

import math

# Exact str: code-point order, including non-ASCII and lone surrogates.
words = ["pear", "Apple", "apple", "éclair", "zebra", "", "a", "ab", "\udcff", "é", "Z", "\U0001f600"]
print("str", [ascii(w) for w in sorted(words)])
print("str reverse", [ascii(w) for w in sorted(words, reverse=True)])

# Inline ints, with negatives and extremes of the inline range.
ints = [5, -3, 0, 2**40, -(2**40), 7, 7, -1]
print("int", sorted(ints))

# Floats: NaN compares false both ways, so its position follows timsort's
# comparisons; inf and -0.0 order as their values.
floats = [3.5, -1.25, float("inf"), -float("inf"), 0.0, -0.0, 2.0, float("nan"), 1e-300]
print("float", [repr(f) for f in sorted(floats)])
print("float no nan", sorted([2.5, -0.5, 1.0, -7.0]))

# Stability: equal keys keep their input order (key= makes keys homogeneous).
records = [("b", 2), ("a", 1), ("c", 2), ("d", 1), ("e", 2)]
print("stable", sorted(records, key=lambda r: r[1]))
print("stable str", sorted(records, key=lambda r: "k"))
print("stable reverse", sorted(records, key=lambda r: r[1], reverse=True))

# Mixed types take the generic comparison: same results.
print("mixed numbers", sorted([3, 1.5, 2, 0.5, True, False]))


class Rank:
    calls = 0

    def __init__(self, value):
        self.value = value

    def __lt__(self, other):
        Rank.calls += 1
        return self.value < other.value

    def __repr__(self):
        return f"Rank({self.value})"


ranked = sorted([Rank(3), Rank(1), Rank(2)])
print("user lt", ranked, Rank.calls > 0)

# A str subclass is not an exact str: generic path, same order.
class Tag(str):
    pass


print("str subclass", sorted([Tag("b"), Tag("a"), Tag("c")]))

# In-place list.sort with a homogeneous key function.
data = list(range(20, 0, -3))
data.sort(key=lambda x: -x)
print("inplace", data)
print("nan in key", sorted([1, 2, 3], key=lambda x: math.nan if x == 2 else float(x)))
