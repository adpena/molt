"""Purpose: differential coverage for nested comprehensions with shadowed vars."""

vals = [[i for i in range(2)] for i in range(2)]
print("vals", vals)

print("i", i)
