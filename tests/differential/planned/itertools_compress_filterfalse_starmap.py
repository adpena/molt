"""Purpose: differential coverage for itertools.compress/filterfalse/starmap."""

import itertools

print(list(itertools.compress(["a", "b", "c"], [1, 0, 1])))
print(list(itertools.filterfalse(lambda x: x % 2, [1, 2, 3, 4])))
print(list(itertools.starmap(pow, [(2, 3), (3, 2)])))
