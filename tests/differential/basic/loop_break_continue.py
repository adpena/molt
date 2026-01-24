"""Purpose: differential coverage for loop break continue."""

values = []
for i in range(5):
    if i == 2:
        break
    values.append(i)
print(values)

values = []
for i in range(5):
    if i == 2:
        continue
    values.append(i)
print(values)

i = 0
while True:
    i += 1
    if i == 3:
        break
print(i)

i = 0
total = 0
while i < 5:
    i += 1
    if i % 2 == 0:
        continue
    total += i
print(total)
