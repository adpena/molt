"""Purpose: differential coverage for itertools.accumulate edge cases."""

import itertools

print(list(itertools.accumulate([], initial=5)))
print(list(itertools.accumulate([1, 2], initial=0)))
