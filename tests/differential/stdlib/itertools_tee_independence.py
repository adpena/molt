"""Purpose: differential coverage for itertools.tee iterator independence."""

import itertools

it1, it2 = itertools.tee([1, 2, 3], 2)
print("a", next(it1))
print("b", next(it1))
print("c", next(it2))
print("d", list(it1))
print("e", list(it2))
