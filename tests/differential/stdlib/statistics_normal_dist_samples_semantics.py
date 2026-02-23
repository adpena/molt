"""Purpose: differential coverage for statistics.NormalDist.samples semantics."""

import random
import statistics

dist = statistics.NormalDist(2.0, 3.0)

seeded_a = [round(value, 12) for value in dist.samples(4, seed=17)]
seeded_b = [round(value, 12) for value in dist.samples(4, seed=17)]
print("seed_repro", seeded_a == seeded_b)

rng = random.Random(17)
expected = [round(dist.inv_cdf(rng.random()), 12) for _ in range(4)]
print("inv_cdf_route", seeded_a == expected)

print("zero_count", dist.samples(0, seed=17))

for label, thunk in [
    ("n_str", lambda: dist.samples("3")),
    ("n_float", lambda: dist.samples(2.5)),
    ("seed_bad", lambda: dist.samples(2, seed=object())),
]:
    try:
        thunk()
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__)

original_random = random.random
random.random = lambda: 0.0
try:
    dist.samples(1)
except Exception as exc:  # noqa: BLE001
    print("zero_probability", type(exc).__name__, str(exc))
finally:
    random.random = original_random
