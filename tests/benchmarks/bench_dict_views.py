data = {}
i = 0
while i < 10_000:
    data[i] = i * 2
    i += 1

total = 0
for key in data.keys():
    total += key
for value in data.values():
    total += value
for pair in data.items():
    total += pair[0] + pair[1]

print(total)
