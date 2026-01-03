lst = [1, 2, 3]
lst.extend([4, 5])
lst.insert(1, 99)
lst.remove(2)
print(lst)

d = {"a": 1, "b": 2}
print(d.pop("a"))
print(d.pop("c", 9))
items = d.items()
pair_sum = 0
for pair in items:
    pair_sum = pair_sum + pair[1]
print(pair_sum)

t = (1, 2, 1)
print(t.count(1))
print(t.index(2))

total = 0
for x in [1, 2, 3]:
    total = total + x
print(total)
acc = 0
for x in (4, 5):
    acc = acc + x
print(acc)

d2 = {1: 10, 2: 20}
sumk = 0
for x in d2.keys():
    sumk = sumk + x
print(sumk)
sumv = 0
for x in d2.values():
    sumv = sumv + x
print(sumv)
