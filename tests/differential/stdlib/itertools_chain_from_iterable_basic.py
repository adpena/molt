"""Purpose: differential coverage for itertools.chain.from_iterable."""

import itertools

print(list(itertools.chain.from_iterable([[1, 2], [3]])))
