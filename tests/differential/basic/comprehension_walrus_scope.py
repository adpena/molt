"""Purpose: differential coverage for walrus in comprehensions and scope."""

x = 0
vals = [x := i for i in range(3) if (x := i) >= 0]
print("vals", vals)
print("x", x)

try:
    i
except Exception as exc:
    print("i", type(exc).__name__)
