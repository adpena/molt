"""Purpose: differential coverage for container methods."""

lst = [1, 2, 3]
print(lst.reverse(), lst)
lst.clear()
print(lst)
lst2 = [1, 2]
lst3 = lst2.copy()
lst2.append(3)
print(lst2, lst3)
lst4 = [0]
lst4.extend(range(3))
print(lst4)

d = {"a": 1}
print(d.setdefault("a", 5), d)
print(d.setdefault("b", 2), d)
d.update({"c": 3})
print(d)
d.update([("d", 4), ("e", 5)])
print(d)
u = d.update
print(u(), d)

s = "AbC"
print(s.lower(), s.upper())
