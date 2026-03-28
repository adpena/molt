"""Purpose: nested indexed loops — tuple collection per pair."""

results = []
for i in range(3):
    for j in range(3):
        results.append((i, j))
print(results)
