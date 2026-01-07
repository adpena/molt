data = b"abc" * 1000
i = 0
total = 0
while i < 1000:
    out = data.replace(b"ab", b"ba")
    total += len(out)
    i += 1

print(total)
