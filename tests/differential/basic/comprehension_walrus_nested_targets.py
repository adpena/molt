"""Purpose: differential coverage for assignment expressions in nested comprehensions."""

x = 0
vals = [(x := i, j) for i in range(2) for j in range(2) if (x := j) >= 0]
print("vals", vals)
print("x", x)
