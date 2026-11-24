"""Purpose: differential coverage for typing paramspec concatenate."""

import sys


typing_loaded = "typing" in sys.modules
from collections.abc import Callable as ABCCallable


shaped = type("ParamSpec", (), {"__module__": "typing"})()
print(ABCCallable[shaped, int].__args__[0] is shaped)
try:
    ABCCallable[object(), int]
except TypeError:
    print("invalid Callable parameter rejected")
print("typing import state preserved:", ("typing" in sys.modules) == typing_loaded)

from typing import Callable, Concatenate, ParamSpec


P = ParamSpec("P")
try:
    P.__typing_subst__(shaped)
except TypeError:
    print("lookalike ParamSpec rejected for substitution")


def wrap(fn: Callable[P, int]) -> Callable[P, int]:
    def inner(*args: P.args, **kwargs: P.kwargs) -> int:
        return fn(*args, **kwargs)

    return inner


def add(a: int, b: int) -> int:
    return a + b


wrapped = wrap(add)
print(wrapped(1, 2))

C = Callable[Concatenate[int, P], int]
print(C)
