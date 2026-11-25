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


def checked_flag(depth, flag):
    if depth < 0:
        raise ValueError("negative depth")
    if depth == 0:
        return flag
    return not checked_flag(depth - 1, flag)


def checked_count(depth, flag):
    if depth < 0:
        raise ValueError("negative count")
    if depth == 0:
        return -2 if flag else 3
    return (checked_count(depth - 1, flag) + 1) & 15


def checked_path(depth):
    try:
        print("checked", depth, checked_flag(depth, False))
    except ValueError as error:
        print("checked error", str(error))
    finally:
        print("checked finally", depth)
    try:
        print("count", depth, checked_count(depth, True) + 5)
    except ValueError as error:
        print("count error", str(error))


print(fib(20), fact(25), type(fact(25)).__name__)
print(fact(30) == 265252859812191058636308480000000)

limit = sys.getrecursionlimit()
try:
    down(0)
except RecursionError:
    print("RecursionError")
print(sys.getrecursionlimit() == limit)

for depth in (0, 1, -1, 2, 23):
    checked_path(depth)
escaped = (checked_flag,)
print("escaped", escaped[0](0, True))
