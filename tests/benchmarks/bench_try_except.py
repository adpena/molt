i = 0
total = 0
while i < 200_000:
    try:
        total += i
    except ValueError:
        total -= 1
    i += 1

print(total)
