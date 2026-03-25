"""Benchmark: string operations."""

N = 100_000
parts = []
i = 0
while i < N:
    parts.append(str(i))
    i = i + 1

result = ",".join(parts)
print(len(result))
