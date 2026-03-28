"""Purpose: triple-nested indexed loops — stress test for loop IR durability."""

results = []
for i in range(2):
    for j in range(2):
        for k in range(2):
            results.append((i, j, k))
print(results)
