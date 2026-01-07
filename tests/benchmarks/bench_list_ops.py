items = []
i = 0
while i < 20_000:
    items.append(i)
    i += 1

total = 0
i = 0
while i < 20_000:
    total += items[i]
    i += 1

print(total)
