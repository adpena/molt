total = 0
for i in range(2000):
    lst = [i, i + 1]
    d = {"x": i}
    total = total + len(lst) + len(d)
print(total)
