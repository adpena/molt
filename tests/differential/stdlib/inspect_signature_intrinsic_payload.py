"""Purpose: exercise inspect.signature lowering payload for full arg shapes."""

import inspect


def f(a, b=2, /, c=3, *args, d, e=5, **kwargs):
    return (a, b, c, args, d, e, kwargs)


def g(x, /, y, *, z):
    return (x, y, z)


print(str(inspect.signature(f)))
print(str(inspect.signature(g)))
