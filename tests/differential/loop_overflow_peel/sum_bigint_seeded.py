def compute(n):
    total = 1237940039285380274899124224
    i = 0
    while i < n:
        total = total + i
        i = i + 1
    return total


print(compute(1000000))
