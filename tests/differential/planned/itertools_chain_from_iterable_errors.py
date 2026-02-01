"""Purpose: differential coverage for itertools.chain.from_iterable errors."""

import itertools

try:
    itertools.chain.from_iterable(5)
except Exception as exc:
    print(type(exc).__name__)
