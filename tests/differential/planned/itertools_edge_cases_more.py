"""Purpose: differential coverage for itertools edge cases."""

import itertools

print("islice", list(itertools.islice(range(10), 2, None, 3)))
print("accumulate", list(itertools.accumulate([1, 2, 3])))

it1, it2 = itertools.tee([1, 2, 3], 2)
print("tee1", list(it1))
print("tee2", list(it2))

pairs = [(k, list(g)) for k, g in itertools.groupby("aaabccc")]
print("groupby", pairs)
