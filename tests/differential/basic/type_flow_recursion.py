"""Type flow (design 78 section 4): recursion keeps CPython's semantics.

`fib` returns an int on every path; an int context must still overflow to big
ints and still raise RecursionError at the interpreter's depth limit.
"""

import sys


def fib(n: int) -> int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)


def fact(n):
    if n <= 1:
        return 1
    return n * fact(n - 1)


def down(n):
    return down(n + 1)


print(fib(20), fact(25), type(fact(25)).__name__)
print(fact(30) == 265252859812191058636308480000000)

limit = sys.getrecursionlimit()
try:
    down(0)
except RecursionError:
    print("RecursionError")
print(sys.getrecursionlimit() == limit)
