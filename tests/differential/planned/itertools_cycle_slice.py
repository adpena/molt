"""Purpose: differential coverage for itertools.cycle + islice behavior."""

import itertools

cycler = itertools.cycle([1, 2])
print("slice", list(itertools.islice(cycler, 5)))
