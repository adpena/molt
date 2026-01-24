"""Purpose: differential coverage for typing paramspec concatenate."""

from typing import Callable, Concatenate, ParamSpec


P = ParamSpec("P")


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
