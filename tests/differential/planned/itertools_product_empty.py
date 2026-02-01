"""Purpose: differential coverage for itertools.product empty input."""

import itertools

print(list(itertools.product([], repeat=2)))
print(list(itertools.product([1, 2], [])))
