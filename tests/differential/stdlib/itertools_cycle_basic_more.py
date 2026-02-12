"""Purpose: differential coverage for itertools.cycle basics."""

import itertools

it = itertools.cycle([1, 2])
print([next(it) for _ in range(5)])
