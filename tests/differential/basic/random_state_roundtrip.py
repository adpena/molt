"""Purpose: differential coverage for random state roundtrip."""

import random

rng = random.Random(123)
state = rng.getstate()
vals1 = [rng.random(), rng.randrange(10), rng.randint(1, 6)]
rng.setstate(state)
vals2 = [rng.random(), rng.randrange(10), rng.randint(1, 6)]
print(vals1)
print(vals2)
print(vals1 == vals2)

seq = [1, 2, 3, 4]
print(rng.choice(seq))
print(rng.choices(seq, k=3))
print(rng.sample(seq, k=2))

try:
    rng.sample(seq, k=10)
except Exception as exc:
    print(type(exc).__name__)
