"""Purpose: differential coverage for inspect getmodule."""

import inspect


def foo():
    return 1


mod = inspect.getmodule(foo)
print(mod.__name__ if mod else None)
