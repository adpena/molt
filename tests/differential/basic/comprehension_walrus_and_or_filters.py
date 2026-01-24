"""Purpose: differential coverage for walrus binding with and/or filters."""

x = 0
vals = [
    (x := i)
    for i in range(3)
    if (x := i) == 1 or (x := 99)
]
print("vals", vals)
print("x", x)
