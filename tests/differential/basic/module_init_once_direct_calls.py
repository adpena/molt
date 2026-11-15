"""Purpose: calls of module functions stored once at initialization (HF-846
slice 2). A call whose callee is the function object of its target's one
def, with an exact positional call of a positional-only signature, enters
the target directly; every other call keeps its guards. Each case checks
the direct path and its neighbours against CPython: a call before the def
raises NameError, recursion and RecursionError, a decorated function, a
defaults tuple with a mutable default, a rewritten __defaults__, a
zero-argument function, keyword arguments, and a module-function loop."""

import functools
import sys


def early_caller():
    return late_target(2)


try:
    early_caller()
except NameError as exc:
    print("before the def", type(exc).__name__)


def late_target(x):
    return x * 3


print("after the def", early_caller())


def fib(n):
    return n if n < 2 else fib(n - 1) + fib(n - 2)


print("fib", fib(20))


def descend(n):
    return descend(n + 1)


sys.setrecursionlimit(200)
try:
    descend(0)
except RecursionError as exc:
    print("recursion", type(exc).__name__)
sys.setrecursionlimit(1000)


def traced(func):
    @functools.wraps(func)
    def wrapper(*args):
        return ("traced", func(*args))

    return wrapper


@traced
def decorated(x):
    return x + 1


def call_decorated(x):
    return decorated(x)


print("decorated", call_decorated(4))


def accumulate(item, bucket=[]):
    bucket.append(item)
    return len(bucket)


def call_accumulate():
    accumulate(1)
    accumulate(2)
    return accumulate(3, [])


print("mutable default", call_accumulate(), accumulate(4))


def scaled(x, factor=2):
    return x * factor


def call_scaled():
    return scaled(5), scaled(5, 10)


print("defaults before", call_scaled())
scaled.__defaults__ = (7,)
print("defaults after", call_scaled())


def zero():
    return "zero"


def call_zero():
    return zero()


print("zero arguments", call_zero())


def keyword(a, b):
    return a - b


def call_keyword():
    return keyword(b=1, a=10), keyword(10, 1)


print("keywords", call_keyword())


def step(total, i):
    return total + i * i


def loop(n):
    total = 0
    for i in range(n):
        total = step(total, i)
    return total


print("loop", loop(10000))
