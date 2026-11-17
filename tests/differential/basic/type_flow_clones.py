"""Type flow (design 78 sections 3.7 and 4): one function, many argument types.

Each call site below passes exact scalars of one family, so a whole-program
type flow may compile one clone of `combine` per family and send each site
to its clone. Every clone is the same Python function: results, result
types, the code object, the qualname and tracebacks raised inside a clone
match CPython, and a site whose argument type varies keeps the generic body.
"""

import traceback


def combine(a, b):
    if a == b:
        return a
    return a + b


def describe(x):
    return x is None, type(x).__name__


def inner(n):
    if n > 2:
        raise ValueError("inner %d" % n)
    return n * 10


def outer(n):
    return inner(n + 1) + 1


print(combine(2, 3), type(combine(2, 3)).__name__)
print(combine(2.5, 0.25), type(combine(2.5, 0.25)).__name__)
print(combine("ab", "cd"), combine("x", "x"))
print(combine(True, True), type(combine(True, True)).__name__)
print(combine(True, False), type(combine(True, False)).__name__)
print(combine(2**62, 2**62), combine(-(2**63), -1))
print(describe(None), describe(1), describe(1.5), describe("s"), describe(False))

# One site, many types: the generic body serves it.
for left, right in [(1, 2), (1.5, 2), ("a", "b"), ([1], [2]), (True, 1)]:
    print(combine(left, right))

# A clone is the original function.
print(combine.__name__, combine.__qualname__, combine.__code__.co_name)
print(combine.__code__.co_firstlineno, combine.__code__.co_argcount)

# A traceback through two clones names the original functions and lines.
print(outer(0), outer(1))
try:
    outer(2)
except ValueError as error:
    frames = traceback.extract_tb(error.__traceback__)
    print(error, [(frame.name, frame.lineno) for frame in frames])
