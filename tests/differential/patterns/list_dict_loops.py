lines = ["alpha", "beta", "gamma", "beta"]

counts = {"alpha": 0, "beta": 0, "gamma": 0}
for i in range(len(lines)):
    name = lines[i]
    counts[name] = counts[name] + 1

print(counts["alpha"])
print(counts["beta"])
print(counts["gamma"])

matrix = [[1, 2], [3, 4], [5, 6]]

acc = 0
for row in matrix:
    for value in row:
        acc = acc + value
print(acc)
