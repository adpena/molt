"""Purpose: differential coverage for itertools.permutations errors."""

import itertools

try:
    list(itertools.permutations([1, 2], -1))
except Exception as exc:
    print(type(exc).__name__)
