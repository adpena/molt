"""Purpose: differential coverage for itertools.accumulate."""

import itertools
import operator

print(list(itertools.accumulate([1, 2, 3])))
print(list(itertools.accumulate([1, 2, 3], operator.mul)))
print(list(itertools.accumulate([1, 2, 3], initial=10)))
