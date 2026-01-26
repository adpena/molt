"""Purpose: differential coverage for nested loops."""

items = [1, 2, 3, 4]

total = 0
for i in range(1, 4):
    for j in range(len(items)):
        total = total + items[j] * i
print(total)

keys = ["a", "b", "c"]
values = [10, 20, 30]

mapping = {}
for i in range(len(keys)):
    mapping[keys[i]] = values[i]
print(mapping["a"])
print(mapping["c"])
