"""Benchmark: float arithmetic 1M iterations."""

N = 1_000_000
x = 0.0
i = 0
while i < N:
    x = x + 1.5
    x = x * 0.99
    i = i + 1
print(x)
