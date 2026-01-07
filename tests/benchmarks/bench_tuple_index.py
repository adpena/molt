t = tuple(range(1000))
total = 0
outer = 0
while outer < 500:
    i = 0
    limit = len(t)
    while i < limit:
        total += t[i]
        i += 1
    outer += 1

print(total)
