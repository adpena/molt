data = tuple(range(10_000))

total = 0
i = 0
while i < 1_000:
    chunk = data[100:9900:3]
    total += len(chunk)
    i += 1

print(total)
