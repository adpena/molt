"""Purpose: differential coverage for itertools.cycle empty input."""

import itertools

print(list(itertools.islice(itertools.cycle([]), 3)))
