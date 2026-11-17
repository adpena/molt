"""Type flow (design 78 section 5.5): an inlined clone keeps CPython's semantics.

`add` called with two ints in a loop may compile to a clone that cannot raise;
such a clone links no frame record and is inlined into its caller. The
generic body still serves every other caller: it raises with its own frame in
the traceback, keeps its code object and qualname, and a recursion that calls
the leaf at every level still raises RecursionError.
"""

import sys
import traceback


def add(a, b):
    return a + b


def kernel(n):
    total = 0
    for i in range(n):
        total = add(total, i)
    return total


def wide(n):
    total = 2**62
    for i in range(n):
        total = add(total, total)
    return total


def climb(n):
    return add(climb(n + 1), 1)


def calls(f):
    return f(3, 4)


print(kernel(1000), kernel(0), type(kernel(10)).__name__)
print(wide(3), wide(70) == 2**132)
print(add(2.5, 1), add("a", "b"), calls(add))
print(add.__name__, add.__qualname__, add.__code__.co_name, add.__code__.co_argcount)

try:
    add("x", 1)
except TypeError:
    frames = traceback.extract_tb(sys.exc_info()[2])
    print("TypeError", [frame.name for frame in frames])

limit = sys.getrecursionlimit()
try:
    climb(0)
except RecursionError:
    print("RecursionError")
print(sys.getrecursionlimit() == limit)
