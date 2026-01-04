haystack = "caf\u00e9" * 2_000_000
needle = "\u00e9"

haystack.count(needle)
total = 0
for _ in range(25):
    total += haystack.count(needle)
print(total)
