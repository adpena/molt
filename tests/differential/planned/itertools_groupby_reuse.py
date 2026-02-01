"""Purpose: differential coverage for itertools.groupby reuse."""

import itertools

items = [1, 1, 2, 2, 2, 3]
for key, group in itertools.groupby(items):
    print(key, list(group))
