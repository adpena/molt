data = bytearray(b"ab" * 5_000)
needle = bytearray(b"ab")
replacement = bytearray(b"ba")

total = 0
i = 0
while i < 1_000:
    data = data.replace(needle, replacement)
    total += len(data)
    i += 1

print(total)
