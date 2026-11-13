"""Mixed int and float arithmetic and true division on boxed operands.

The late lowering stage (HF-821) gives `+ - * / < ==` on a boxed int and a
boxed float an arm that converts the int exactly, and true division an arm
that divides two ints in floating point. Values arrive as parameters, so the
compiler cannot prove their types and the boxed arms run. CPython's output is
the oracle: each result prints with repr and its type.
"""

import math


def show(label, value):
    print(label, type(value).__name__, repr(value))


def arith(a, b):
    return a + b, a - b, a * b


def compare(a, b):
    return a < b, a <= b, a == b, a != b, a > b, a >= b


def divide(a, b):
    try:
        return a / b
    except ZeroDivisionError as error:
        return "ZeroDivisionError: " + str(error)


class MyInt(int):
    pass


class MyFloat(float):
    pass


INLINE_MAX = 2**46 - 1
PAIRS = [
    (3, 0.5),
    (0.5, 3),
    (-7, 2.25),
    (2.25, -7),
    (INLINE_MAX, 1.0),
    (1.0, INLINE_MAX),
    (-(2**46), 0.25),
    (2**53 + 1, 1.0),
    (1.0, 2**53 + 1),
    (10**30, 2.5),
    (0, -0.0),
    (-0.0, 0),
    (5, math.inf),
    (math.inf, -5),
    (7, math.nan),
    (math.nan, 7),
    (True, 0.5),
    (0.5, False),
    (MyInt(4), 0.5),
    (4, MyFloat(0.5)),
]

for a, b in PAIRS:
    label = f"{a!r},{b!r}"
    for name, value in zip(("add", "sub", "mul"), arith(a, b)):
        show(f"{name} {label}", value)
    print("cmp", label, compare(a, b))
    show(f"div {label}", divide(a, b))

INT_DIVS = [
    (1, 3),
    (7, 2),
    (-7, 2),
    (7, -2),
    (2**45, 3),
    (INLINE_MAX, 7),
    (-(2**46), 3),
    (10**15, 7),
    (2**53 + 1, 3),
    (10**30, 7),
    (0, 5),
    (5, 0),
    (0, 0),
    (True, 2),
    (MyInt(9), 4),
]
for a, b in INT_DIVS:
    show(f"intdiv {a!r},{b!r}", divide(a, b))

for a, b in [(1.0, 0.0), (1, 0.0), (1.0, 0), (2.5, -0.0), (math.nan, 0.0), (0.0, 0)]:
    show(f"zero {a!r},{b!r}", divide(a, b))

# Exact equality of an inline int and its float, both sides of the window.
for n in [2**46 - 1, -(2**46), 2**53, 2**53 + 1]:
    print("eq", n, n == float(n), float(n) == n, n < float(n) + 1)

total = 0
for i in range(1, 6):
    total = total + i / 2
    total = total - i * 0.25
show("loop", total)

x = 9
x /= 4
show("inplace", x)
y = 9.0
y /= 4
show("inplace float", y)
