"""Purpose: differential coverage for tuple keys."""

print((1, 2, 3))
print(len((1, 2, 3)))
print((1, 2, 3)[1])
print((1,))

d = {(1, 2): "a", (1, 3): "b"}
print(d[(1, 2)])
print(d.get((1, 4)))
print(d.get((1, 4), "z"))
