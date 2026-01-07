total = 0
i = 0
while i < 200_000:
    t = (i, i + 1, i + 2, i + 3)
    total += t[0] + t[1] + t[2] + t[3]
    i += 1

print(total)
