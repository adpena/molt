# MOLT_META: min_py=3.12
"""Purpose: differential coverage for itertools.batched."""

import itertools

print(list(itertools.batched([1, 2, 3, 4, 5], 2)))
