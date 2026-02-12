"""Purpose: differential coverage for itertools.groupby key function."""

import itertools

items = ["aa", "ab", "bb", "bc"]
for key, group in itertools.groupby(items, key=lambda s: s[0]):
    print(key, list(group))
