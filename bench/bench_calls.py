"""Benchmark: function calls 1M."""


def inc(x):
    return x + 1


N = 1_000_000
total = 0
i = 0
while i < N:
    total = inc(total)
    i = i + 1
print(total)
