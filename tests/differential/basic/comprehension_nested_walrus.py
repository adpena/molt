"""Purpose: differential coverage for nested comprehension walrus scoping."""

x = 0
vals = [[(x := i), (x := j)] for i in range(2) for j in range(2)]
print("vals", vals)
print("x", x)
