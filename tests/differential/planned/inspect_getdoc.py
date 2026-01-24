"""Purpose: differential coverage for inspect getdoc."""

import inspect


def foo():
    """docstring"""
    return 1


print(inspect.getdoc(foo))
