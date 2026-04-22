# Parity test: collections (list, dict, set, tuple)
# All output via print() for diff comparison

print("=== List basics ===")
a = [1, 2, 3, 4, 5]
print(a)
print(len(a))
print(a[0])
print(a[-1])
print(a[1:3])
print(a[::-1])

print("=== List mutation ===")
a = [1, 2, 3]
a.append(4)
print(a)
a.insert(0, 0)
print(a)
a.extend([5, 6])
print(a)
x = a.pop()
print(x, a)
x = a.pop(0)
print(x, a)
a.remove(3)
print(a)

print("=== List sort/reverse ===")
a = [3, 1, 4, 1, 5, 9, 2, 6]
b = sorted(a)
print(b)
print(sorted(a, reverse=True))
a.sort()
print(a)
a.reverse()
print(a)

print("=== List comprehension ===")
print([x * 2 for x in range(5)])
print([x for x in range(10) if x % 2 == 0])
print([x * y for x in range(3) for y in range(3)])

print("=== List misc ===")
a = [1, 2, 3, 2, 1]
print(a.count(2))
print(a.index(2))
print(2 in a)
print(10 in a)
b = a.copy()
b.append(99)
print(a)
print(b)
a.clear()
print(a)

print("=== List concatenation ===")
print([1, 2] + [3, 4])
print([0] * 5)
print([] + [])
print([1] * 0)

print("=== Dict basics ===")
d = {"a": 1, "b": 2, "c": 3}
print(d["a"])
print(d.get("b"))
print(d.get("z", "default"))
print(len(d))
print("a" in d)
print("z" in d)

print("=== Dict mutation ===")
d = {"a": 1}
d["b"] = 2
print(d)
d.update({"c": 3, "d": 4})
print(d)
x = d.pop("b")
print(x, d)
d.setdefault("e", 5)
print(d)
d.setdefault("e", 99)
print(d)

print("=== Dict views ===")
d = {"a": 1, "b": 2, "c": 3}
print(sorted(d.keys()))
print(sorted(d.values()))
print(sorted(d.items()))

print("=== Dict comprehension ===")
print({x: x**2 for x in range(5)})
print({k: v for k, v in [("a", 1), ("b", 2)]})

print("=== Dict from keys ===")
print(dict.fromkeys(["a", "b", "c"], 0))
print(dict.fromkeys(range(3)))

print("=== Dict merge (PEP 584) ===")
d1 = {"a": 1, "b": 2}
d2 = {"b": 3, "c": 4}
print({**d1, **d2})
print(d1 | d2)
print(d2 | d1)

print("=== Set basics ===")
s = {1, 2, 3, 4, 5}
print(sorted(s))
print(len(s))
print(3 in s)
print(10 in s)

print("=== Set operations ===")
a = {1, 2, 3, 4}
b = {3, 4, 5, 6}
print(sorted(a | b))
print(sorted(a & b))
print(sorted(a - b))
print(sorted(a ^ b))
print(sorted(a.union(b)))
print(sorted(a.intersection(b)))
print(sorted(a.difference(b)))
print(sorted(a.symmetric_difference(b)))

print("=== Set mutation ===")
s = {1, 2, 3}
s.add(4)
print(sorted(s))
s.discard(2)
print(sorted(s))
s.discard(99)
print(sorted(s))

print("=== Set predicates ===")
print({1, 2}.issubset({1, 2, 3}))
print({1, 2, 3}.issuperset({1, 2}))
print({1, 2}.isdisjoint({3, 4}))
print({1, 2}.isdisjoint({2, 3}))

print("=== Set comprehension ===")
print(sorted({x % 3 for x in range(10)}))

print("=== Frozenset ===")
fs = frozenset([1, 2, 3, 2, 1])
print(sorted(fs))
print(len(fs))
print(2 in fs)

print("=== Tuple basics ===")
t = (1, 2, 3, 4, 5)
print(t)
print(len(t))
print(t[0])
print(t[-1])
print(t[1:3])
print(t[::-1])

print("=== Tuple operations ===")
print((1, 2) + (3, 4))
print((1, 2) * 3)
print(2 in (1, 2, 3))
print((1, 2, 3, 2).count(2))
print((1, 2, 3, 2).index(2))

print("=== Nested structures ===")
nested = [[1, 2], [3, 4], [5, 6]]
print(nested)
print(nested[1])
print(nested[1][0])
d = {"a": [1, 2, 3], "b": {"c": 4}}
print(d["a"])
print(d["b"]["c"])

print("=== Unpacking ===")
a, b, c = [1, 2, 3]
print(a, b, c)
first, *rest = [1, 2, 3, 4, 5]
print(first, rest)
*init, last = [1, 2, 3, 4, 5]
print(init, last)
a, *mid, z = [1, 2, 3, 4, 5]
print(a, mid, z)

print("=== Empty collections ===")
print([])
print({})
print(set())
print(())
print(len([]))
print(len({}))
print(len(set()))
print(len(()))
