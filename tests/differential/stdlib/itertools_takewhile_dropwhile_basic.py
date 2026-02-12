"""Purpose: differential coverage for itertools.takewhile/dropwhile."""

import itertools

print(list(itertools.takewhile(lambda x: x < 3, [1, 2, 3, 1])))
print(list(itertools.dropwhile(lambda x: x < 3, [1, 2, 3, 1])))
