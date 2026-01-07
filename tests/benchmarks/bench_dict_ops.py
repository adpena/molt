d = {}
i = 0
while i < 20_000:
    d[i] = i + 1
    i += 1

total = 0
i = 0
while i < 20_000:
    total += d[i]
    i += 1

print(total)
