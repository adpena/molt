"""Purpose: differential coverage for inspect basic."""

import inspect


def foo(a, b=1, *args, **kwargs):
    return a + b


sig = inspect.signature(foo)
print(str(sig))

print(inspect.isfunction(foo))
print(inspect.isroutine(foo))
