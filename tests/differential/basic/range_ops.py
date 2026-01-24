"""Purpose: differential coverage for range ops."""

print(len(range(5)))
print(range(5)[0])
print(range(5)[-1])

total = 0
for i in range(3):
    total = total + i
print(total)

acc = 0
for i in range(5, 0, -2):
    acc = acc + i
print(acc)

r = range(5, 0, -2)
print(len(r))
print(r[0])
print(r[1])

lst = list(range(1, 6, 2))
print(len(lst))
print(lst[0])
print(lst[-1])
