"""Purpose: a guarded call keeps CPython semantics when the compiled target is
called directly (HF-821 D4): rebinding and __code__ replacement, defaults and
wrong arity, closures, temporaries as callables, callables live in a loop,
exceptions and finally through the call, None and raw returns, and the
recursion limit."""

import sys


def add(a, b):
    return a + b


def nothing(a):
    pass


def ident(x):
    return x


def with_default(a, b=10):
    return a + b


def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)


print(add(1, 2), add("a", "b"), nothing(1), ident(2**70), ident(1.5), with_default(1), with_default(1, 2))
print(fib(15))

total = 0
for i in range(50):
    total = add(total, i)
print(total)


def make(k):
    def inner(x):
        return x + k

    return inner


print(make(3)(4), [make(i)(i) for i in range(4)])

try:
    add(1)
except TypeError as error:
    print("TypeError", error)
try:
    add(1, 2, 3)
except TypeError as error:
    print("TypeError", error)


def boom(x):
    raise ValueError(f"boom {x}")


def guarded(x):
    log = []
    try:
        log.append(ident(x))
        boom(x)
        log.append("unreached")
    except ValueError as error:
        log.append(str(error))
    finally:
        log.append("finally")
    return log


print(guarded(7))

# Rebinding the global, then replacing the function's code: the guard refuses
# and the call takes what the name holds now.
saved = add


def other_add(a, b):
    return ("other", a, b)


add = other_add
print(add(1, 2))
add = saved
print(add(1, 2))


def replacement(a, b):
    return ("replaced", a - b)


add.__code__ = replacement.__code__
print(add(5, 3), saved(5, 3))


class Callable:
    def __call__(self, a, b):
        return ("instance", a, b)


add = Callable()
print(add(1, 2))
add = lambda a, b: a * b  # noqa: E731
print(add(3, 4))


def depth(n):
    return 1 + depth(n + 1)


sys.setrecursionlimit(200)
try:
    depth(0)
except RecursionError as error:
    print("RecursionError", type(error).__name__)
finally:
    print("after recursion")
print(fib(10), ident("still works"))
