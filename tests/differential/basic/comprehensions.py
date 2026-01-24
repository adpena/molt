"""Purpose: differential coverage for comprehensions."""

print([x * 2 for x in range(3)])
print(sorted({x for x in [1, 2, 2, 3]}))
print({k: v for k, v in [("a", 1), ("b", 2), ("a", 3)]})
print([a + b for (a, b) in [(1, 2), (3, 4)]])
print([i * j for i in range(3) for j in range(2) if j])

x = "outer"
vals = [x for x in [1, 2, 3]]
print(vals)
print(x)


def make_gen():
    x = 1
    gen = (x for _ in range(2))
    x = 5
    return list(gen)


print(make_gen())

x = 1
gen = (x for _ in range(2))
x = 7
print(list(gen))

x = "scope"
_ = [x for x in range(2)]
print(x)
