"""Purpose: differential coverage for statistics.NormalDist behavior."""

import statistics

n = statistics.NormalDist(2.0, 3.0)
m = statistics.NormalDist(1.0, 4.0)

print(round(n.mean, 6), round(n.stdev, 6), round(n.variance, 6))
print(round(n.pdf(2.0), 12))
print(round(n.cdf(2.0), 12))
print(round(n.inv_cdf(0.5), 12))
print([round(x, 6) for x in n.quantiles(4)])
print(round(n.zscore(5.0), 12))
print(round(n.overlap(m), 12))

print(repr(n + m))
print(repr(n - m))
print(repr(n * 2))
print(repr(2 + n))
print(repr(10 - n))

from_samples = statistics.NormalDist.from_samples([1.0, 2.0, 3.0, 4.0])
print(round(from_samples.mean, 12), round(from_samples.stdev, 12))

print(n == statistics.NormalDist(2.0, 3.0))
print(hash(n) == hash(statistics.NormalDist(2.0, 3.0)))

for thunk in [
    lambda: statistics.NormalDist(0.0, -1.0),
    lambda: statistics.NormalDist(0.0, 0.0).pdf(1.0),
    lambda: statistics.NormalDist(0.0, 0.0).cdf(1.0),
    lambda: statistics.NormalDist(0.0, 0.0).zscore(1.0),
    lambda: n.inv_cdf(0.0),
    lambda: n.overlap(1.0),
]:
    try:
        thunk()
    except Exception as exc:  # noqa: BLE001
        print(type(exc).__name__)
