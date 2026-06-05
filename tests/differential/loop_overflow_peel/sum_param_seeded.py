def compute(n, z):
    total = z
    i = z
    while i < n:
        total = total + i
        i = i + 1
    return total


print(compute(2000000, 0))
