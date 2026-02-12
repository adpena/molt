"""Purpose: differential coverage for inspect getclosurevars."""

import inspect


def outer():
    x = 10

    def inner(y):
        return x + y

    return inner


fn = outer()
vars = inspect.getclosurevars(fn)
print(vars.nonlocals)
