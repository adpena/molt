data = bytearray(b"ab" * 5_000)
needle = bytearray(b"ab")

total = 0
i = 0
while i < 10_000:
    total += data.find(needle)
    i += 1

print(total)
