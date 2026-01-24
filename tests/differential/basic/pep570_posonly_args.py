"""Purpose: differential coverage for PEP 570 positional-only parameters."""


def f(a, b, /, c, *, d):
    return a, b, c, d


def g(a, /, b=2, *, c=3):
    return a, b, c


print(f(1, 2, 3, d=4))
print(g(10))

try:
    f(a=1, b=2, c=3, d=4)
except TypeError as exc:
    print(type(exc).__name__, exc)

try:
    g(a=1)
except TypeError as exc:
    print(type(exc).__name__, exc)
