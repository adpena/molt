# Parity test: comprehensions
# All output via print() for diff comparison

print("=== List comprehension ===")
print([x for x in range(10)])
print([x * 2 for x in range(5)])
print([x for x in range(20) if x % 3 == 0])
print([x ** 2 for x in range(10) if x % 2 == 0])

print("=== Nested list comprehension ===")
print([x * y for x in range(1, 4) for y in range(1, 4)])
print([(x, y) for x in range(3) for y in range(3) if x != y])
print([[j for j in range(3)] for i in range(3)])

print("=== List comp with function calls ===")
print([len(s) for s in ["hello", "world", "hi"]])
print([s.upper() for s in ["hello", "world"]])
print([int(x) for x in ["1", "2", "3"]])

print("=== Dict comprehension ===")
print({x: x ** 2 for x in range(5)})
print({k: v for k, v in zip("abc", [1, 2, 3])})
print({k: v for k, v in enumerate("xyz")})
print({x: x for x in range(5) if x % 2 == 0})

print("=== Dict comp from list ===")
words = ["hello", "world", "hi", "bye"]
print({w: len(w) for w in words})
print({w[0]: w for w in words})

print("=== Set comprehension ===")
print(sorted({x % 5 for x in range(20)}))
print(sorted({len(w) for w in ["a", "bb", "ccc", "dd", "e"]}))
print(sorted({x for x in range(10) if x % 3 == 0}))

print("=== Generator expression ===")
g = (x * x for x in range(5))
print(list(g))
print(sum(x for x in range(10)))
print(min(x * x for x in range(-5, 5)))
print(max(abs(x) for x in [-3, 1, -5, 2]))
print(any(x > 3 for x in [1, 2, 3, 4, 5]))
print(all(x > 0 for x in [1, 2, 3]))
print(all(x > 0 for x in [1, 0, 3]))

print("=== Nested generator ===")
flat = list(x for sub in [[1, 2], [3, 4], [5]] for x in sub)
print(flat)

print("=== Comprehension with walrus ===")
result = [y for x in range(10) if (y := x * x) > 20]
print(result)

print("=== Conditional expression in comp ===")
print([x if x % 2 == 0 else -x for x in range(10)])
print({x: "even" if x % 2 == 0 else "odd" for x in range(5)})

print("=== Comprehension variable scoping ===")
x = 99
result = [x for x in range(5)]
print(result)
print(x)  # x should be 99 (comp has own scope in Python 3)

print("=== Tuple in comprehension ===")
pairs = [(1, "a"), (2, "b"), (3, "c")]
print([f"{n}:{s}" for n, s in pairs])
print({n: s for n, s in pairs})

print("=== Chained operations ===")
data = [1, -2, 3, -4, 5, -6]
print(sorted(x for x in data if x > 0))
print(list(map(abs, filter(lambda x: x < 0, data))))

print("=== Multiple iterables ===")
print([(x, y, z) for x in range(2) for y in range(2) for z in range(2)])

print("=== String comprehension ===")
print("".join(c.upper() if c in "aeiou" else c for c in "hello world"))

print("=== Dict comp with complex keys ===")
print({(x, y): x + y for x in range(3) for y in range(3)})

print("=== Comp over dict ===")
d = {"a": 1, "b": 2, "c": 3}
print(sorted([k for k in d]))
print(sorted([v for v in d.values()]))
print(sorted([(k, v) for k, v in d.items()]))

print("=== Empty comprehensions ===")
print([x for x in []])
print({x: x for x in []})
print(sorted({x for x in []}))
print(list(x for x in []))

print("=== Comp with enumerate ===")
words = ["hello", "world", "test"]
print([(i, w) for i, w in enumerate(words)])
print({i: w for i, w in enumerate(words)})

print("=== Nested comp flattening ===")
matrix = [[1, 2, 3], [4, 5, 6], [7, 8, 9]]
print([x for row in matrix for x in row])
print([[row[i] for row in matrix] for i in range(3)])  # transpose

print("=== Comp with zip ===")
keys = ["a", "b", "c"]
vals = [1, 2, 3]
print({k: v for k, v in zip(keys, vals)})
print([(k, v) for k, v in zip(keys, vals)])
