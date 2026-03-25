"""Benchmark: list append + sum."""

N = 1_000_000
lst = []
i = 0
while i < N:
    lst.append(i)
    i = i + 1

total = 0
i = 0
while i < N:
    total = total + lst[i]
    i = i + 1
print(total)
