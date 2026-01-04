words = ["molt", "runtime", "compiler"]

print("-".join(words))

upper = []
for w in words:
    upper.append(w.replace("o", "O"))
print(upper)

count = 0
for w in words:
    count = count + w.count("o")
print(count)

positions = []
for w in words:
    positions.append(w.find("t"))
print(positions)
