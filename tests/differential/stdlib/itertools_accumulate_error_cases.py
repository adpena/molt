"""Purpose: differential coverage for itertools.accumulate error cases."""

import itertools

try:
    list(itertools.accumulate([1, 2], object()))
except Exception as exc:
    print(type(exc).__name__)
