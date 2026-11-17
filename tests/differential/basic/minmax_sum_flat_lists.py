"""Purpose: min, max and sum over int, bool and float lists keep CPython's results.

Molt reads a list stored as raw ints or bools directly and compares exact ints
and floats as machine values; ties, NaN, signed zero, empty inputs, overflow
past 64 bits and user comparison methods must still behave as CPython's.
"""

import math


class Loud(int):
    def __lt__(self, other):
        print("Loud.__lt__", int(self), int(other))
        return int.__lt__(self, other)

    def __gt__(self, other):
        print("Loud.__gt__", int(self), int(other))
        return int.__gt__(self, other)


class MyList(list):
    def __iter__(self):
        print("MyList.__iter__")
        return iter([100, -100])


ints = list(range(-5, 1000, 7))
print(min(ints), max(ints), sum(ints), sum(ints, 10))
print(min([3]), max([3]), sum([]), sum([], 5))
print(min([5, 1, 1, 5]), max([5, 9, 9, 1]))
big = [2**62, 2**62, 2**62, -1]
print(sum(big), min(big), max(big))
print(sum([2**63 - 1, 1]), sum([-(2**63), -1]))
bools = [True, False, True, False]
print(min(bools), max(bools), sum(bools), type(max(bools)).__name__)
print(max([False, False]), min([True]))

for name, fn in (("min", min), ("max", max)):
    try:
        fn([])
    except ValueError as exc:
        print(name, "ValueError", exc)
    print(name, "default", fn([], default="d"), fn([], default=None))

floats = [0.0, -0.0]
print(repr(max(floats)), repr(max(floats[::-1])), repr(min(floats)), repr(min(floats[::-1])))
nan = float("nan")
print(max([nan, 1.0]), max([1.0, nan]), min([nan, -1.0]), min([-1.0, nan]))
print(max([1, 2.5, 2]), min([3, 1.5, 1]), max([1, 1.0]), type(max([1.0, 1])).__name__)
print(math.isnan(max([nan, nan])))

print(max([Loud(3), Loud(7), Loud(5)]), min([Loud(3), Loud(1)]))
print(max(MyList([1, 2, 3])), min(MyList([1, 2, 3])), sum(MyList([1, 2])))
print(max(ints, key=lambda v: -v), min(ints, key=abs))
print(max((4, 9, 2)), min(range(10, 0, -3)), max("hello"))
