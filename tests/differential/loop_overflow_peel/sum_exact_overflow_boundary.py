def compute(n):
    total = 9223372036854775804
    i = 0
    while i < n:
        total = total + i
        i = i + 1
    return total


print(compute(10))
