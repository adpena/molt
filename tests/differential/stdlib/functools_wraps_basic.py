"""Purpose: differential coverage for functools wraps/update_wrapper."""

import functools


def base(a, b):
    """base doc"""
    return a + b


def deco(fn):
    @functools.wraps(fn)
    def wrapper(*args, **kwargs):
        return fn(*args, **kwargs)

    return wrapper


wrapped = deco(base)
print(wrapped.__name__)
print(wrapped.__doc__)
print(wrapped(1, 2))
print(wrapped.__wrapped__ is base)
