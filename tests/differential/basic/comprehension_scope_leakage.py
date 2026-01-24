"""Purpose: differential coverage for comprehension scope leakage across nested scopes."""

x = 100
vals = [x for x in range(3)]
print("vals", vals)
print("x", x)


def outer():
    y = 0
    vals = [y for y in range(2)]
    return vals, y


print("outer", outer())
