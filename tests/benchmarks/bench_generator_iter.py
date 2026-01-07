def gen(n: int):
    i = 0
    while i < n:
        yield i
        i += 1


total = 0
outer = 0
while outer < 100:
    for val in gen(200):
        total += val
    outer += 1

print(total)
