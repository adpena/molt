"""Purpose: differential coverage for itertools.zip_longest."""

import itertools

print(list(itertools.zip_longest([1, 2], [3], fillvalue=0)))
