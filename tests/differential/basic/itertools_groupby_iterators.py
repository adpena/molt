"""Purpose: differential coverage for itertools.groupby iterator consumption."""

import itertools

it = iter("aaab")
key, group = next(itertools.groupby(it))
print("key", key)
print("group", list(group))
print("rest", list(it))
