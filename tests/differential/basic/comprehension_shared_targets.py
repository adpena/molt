"""Purpose: differential coverage for nested comprehensions sharing target names."""

vals = [[i for i in range(2)] for i in range(2)]
print("vals", vals)

try:
    print("i", i)
except Exception as exc:
    print("i", type(exc).__name__)
