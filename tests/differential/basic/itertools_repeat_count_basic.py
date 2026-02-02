"""Purpose: differential coverage for itertools.repeat/count."""

import itertools

print(list(itertools.islice(itertools.repeat("x"), 3)))
print(list(itertools.islice(itertools.count(5, 2), 3)))
