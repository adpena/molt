"""Differential coverage for stable module function aliases."""


def f(x):
    return x * 2


def g():
    h = f
    return h(5) + h(6)


lf = f
print(g())
print(lf(3))
print(f(7))
