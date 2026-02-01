"""Purpose: differential coverage for functools.partial kwargs merging."""

import functools


def f(a, b=0, c=0):
    return a + b + c

p = functools.partial(f, 1, c=3)
print(p(b=2))
print(p())
