"""Purpose: differential coverage for itertools.chain.from_iterable."""

import itertools

nested = [[1, 2], [3], []]
print("chain", list(itertools.chain.from_iterable(nested)))
