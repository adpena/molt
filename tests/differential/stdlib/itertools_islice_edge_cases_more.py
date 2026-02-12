"""Purpose: differential coverage for itertools.islice edge cases."""

import itertools

print(list(itertools.islice([1, 2, 3], 0, None, 2)))
print(list(itertools.islice([1, 2, 3], 10)))
