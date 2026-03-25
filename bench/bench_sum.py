"""Benchmark: sum 10M integers."""

N = 10_000_000
total = 0
i = 0
while i < N:
    total = total + i
    i = i + 1
print(total)
