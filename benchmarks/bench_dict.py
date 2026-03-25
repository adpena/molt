"""Benchmark: dict 1M set + get."""

N = 1_000_000
d = {}
i = 0
while i < N:
    d[i] = i
    i = i + 1

total = 0
i = 0
while i < N:
    total = total + d[i]
    i = i + 1
print(total)
