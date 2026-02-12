"""Purpose: differential coverage for inspect unwrap."""

import functools
import inspect


def deco(fn):
    @functools.wraps(fn)
    def inner(*args, **kwargs):
        return fn(*args, **kwargs)

    return inner


@deco
def foo():
    return 1


print(inspect.unwrap(foo) is foo.__wrapped__)
