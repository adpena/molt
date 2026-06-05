def compute(n):
    total = 0
    i = 0
    while i < n:
        total = total + i * i
        i = i + 1
    return total


print(compute(2000000))
