"""Purpose: differential coverage for itertools.chain error cases."""

import itertools

try:
    itertools.chain(1)
except Exception as exc:
    print(type(exc).__name__)
