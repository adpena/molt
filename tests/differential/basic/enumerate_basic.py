print(list(enumerate(["a", "b"])))
print(list(enumerate(range(3), 5)))

items = []
for idx, val in enumerate([10, 20, 30], start=1):
    items.append((idx, val))
print(items)
