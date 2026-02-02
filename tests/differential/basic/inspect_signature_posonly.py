"""Purpose: differential coverage for inspect signature posonly."""

import inspect


def foo(a, /, b, *, c):
    return a + b + c


print(str(inspect.signature(foo)))
