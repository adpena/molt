"""Purpose: differential coverage for itertools.tee independence."""

import itertools

it1, it2 = itertools.tee([1, 2, 3])
print(next(it1))
print(list(it2))
print(list(it1))
