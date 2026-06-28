"""Stable function aliases must survive comprehension and closure capture."""


def f(i, j):
    return i * j + 1.0


def use_listcomp(u):
    lf = f
    return [sum([lf(i, x) for i, x in enumerate(u)]) for _ in u]


def use_genexpr(u):
    lf = f
    return [sum(lf(i, x) for i, x in enumerate(u)) for _ in u]


def use_nested_fn(u):
    lf = f

    def inner(i):
        return lf(i, i) + lf(i, i + 1)

    return [inner(k) for k in range(len(u))]


data = [1.0, 2.0, 3.0, 4.0]
print(use_listcomp(data))
print(use_genexpr(data))
print(use_nested_fn(data))
print(use_listcomp(data) == use_genexpr(data))
