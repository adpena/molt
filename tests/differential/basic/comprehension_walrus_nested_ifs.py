"""Purpose: differential coverage for walrus binding with nested if filters."""

x = 0
vals = [
    (x := i)
    for i in range(3)
    if (x := i) >= 0
    if (x := i) < 2
]
print("vals", vals)
print("x", x)
