"""Purpose: differential coverage for inspect getsource basic."""

import inspect


def foo():
    return 1


try:
    print(inspect.getsource(foo).strip().splitlines()[-1])
except Exception as exc:
    print(type(exc).__name__)
