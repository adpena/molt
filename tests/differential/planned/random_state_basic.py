"""Purpose: differential coverage for random.Random basics."""

import random

rng = random.Random(123)
print(rng.randrange(10))
print(rng.randint(1, 3))
items = [1, 2, 3]
rng.shuffle(items)
print(items)
