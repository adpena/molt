haystack = "caf\u00e9" * 2_000_000
needle = "\u00e9"

haystack.find(needle)
total = 0
for _ in range(25):
    total += haystack.find(needle)
print(total)
