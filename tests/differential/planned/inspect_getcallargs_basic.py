"""Purpose: ensure inspect.getcallargs maps arguments correctly."""

import inspect


def foo(a, b=2, *args, **kwargs):
    return a + b


print(inspect.getcallargs(foo, 1, 3, 4, x=5))
