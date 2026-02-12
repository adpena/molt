"""Purpose: differential coverage for itertools product/permutations/combinations."""

import itertools

print(list(itertools.product([1, 2], repeat=2)))
print(list(itertools.permutations([1, 2, 3], 2)))
print(list(itertools.combinations([1, 2, 3], 2)))
