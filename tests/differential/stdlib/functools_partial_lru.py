"""Purpose: differential coverage for functools partial lru."""

import functools


def add(a, b, c=0, d=0):
    return a + b + c + d


p = functools.partial(add, 1, d=4)
print(p(2, c=3))
print(p(2, d=5))

p2 = functools.partial(add, 1, 2, 3, 4)
print(p2())

calls = [0]


@functools.lru_cache(maxsize=None)
def f(a, b=0, **kw):
    """cache doc"""
    calls[0] = calls[0] + 1
    return (a, b, kw.get("x", None))


print(f(1, b=2, x=3))
print(f(1, b=2, x=3))
print(calls[0])
print(f"lru_name:{f.__name__}")
print(f"lru_doc:{f.__doc__}")
print(f"lru_wrapped_name:{f.__wrapped__.__name__}")
print(f"lru_wrapped_is_self:{f.__wrapped__ is f}")
