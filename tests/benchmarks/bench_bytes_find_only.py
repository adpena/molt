haystack = b"a" * 10_000_000 + b"b"
needle = b"b"
i = 0
total = 0
while i < 200:
    total = total + haystack.find(needle)
    i = i + 1
print(total)
