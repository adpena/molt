lst = [1, 2, 3]
print(lst[0])
lst[1] = 5
print(lst)
print(lst[-1])
print(lst[1:3])

d = {"a": 1, "b": 2}
print(d["a"])
d["c"] = 3
print(d)

print(lst.append(4))
print(lst)
print(lst.pop())
print(lst)
print(lst.pop(0))
print(lst)
print(lst.count(5))
print(lst.index(5))

print(d.get("b"))
print(d.get("missing"))
print(d.get("missing", 9))
print(len(d.keys()))
print(len(d.values()))

big = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19]
big.append(20)
print(len(big))
