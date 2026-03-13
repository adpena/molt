"""Purpose: differential coverage for ifexp."""


def pick(a, b, cond):
    return a if cond else b


print(pick(1, 2, True))
print(pick(1, 2, False))
print(pick("hello", "world", True))
print(pick("hello", "world", False))
print(pick(3.14, 2.71, True))
print(pick(3.14, 2.71, False))

hits = []


def t():
    hits.append("t")
    return 1


def f():
    hits.append("f")
    return 2


print(t() if True else f())
print(hits)

hits = []
print(t() if False else f())
print(hits)

# Nested ifexp
x = 1 if True else (2 if False else 3)
print(x)
y = 1 if False else (2 if True else 3)
print(y)
z = 1 if False else (2 if False else 3)
print(z)

# Ifexp with expressions (not just variable refs)
print(1 + 1 if True else 2 + 2)
print(1 + 1 if False else 2 + 2)
