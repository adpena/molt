"""Purpose: differential coverage for walrus binding with short-circuit filters."""

x = 0
vals = [
    (x := i)
    for i in range(3)
    if (x := i) == 1 or False
]
print("vals", vals)
print("x", x)
