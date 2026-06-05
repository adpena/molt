def compute(n):
    total = 0
    i = 0
    try:
        while i < n:
            total = total + i
            i = i + 1
    except ValueError:
        total = -1
    return total


print(compute(2000000))
