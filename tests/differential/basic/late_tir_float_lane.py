"""Purpose: float +, -, * and comparisons on boxed words keep CPython semantics
in the late-TIR float arm (HF-821): signed zero, infinities, NaN in every
comparison and in truth tests, mixed int and float operands, bools, int and
float subclasses, and values read from lists (boxed at the operation)."""

import math

NAN = float("nan")
INF = float("inf")
VALUES = [0.0, -0.0, 1.5, -2.25, 1e308, -1e308, INF, -INF, NAN, 3, -7, True, 2**60]


def add(a, b):
    return a + b


def sub(a, b):
    return a - b


def mul(a, b):
    return a * b


def iadd(a, b):
    a += b
    return a


def imul(a, b):
    a *= b
    return a


def show(x):
    if isinstance(x, float):
        if math.isnan(x):
            return "nan"
        return repr(x) + ("(neg)" if math.copysign(1.0, x) < 0 else "")
    return repr(x)


for a in VALUES:
    for b in VALUES:
        print(
            show(a), show(b),
            show(add(a, b)), show(sub(a, b)), show(mul(a, b)),
            show(iadd(a, b)), show(imul(a, b)),
            a == b, a != b, a < b, a <= b, a > b, a >= b,
        )


def truth(x):
    return bool(x), not x, (1 if x else 0)


for x in (0.0, -0.0, NAN, INF, 1e-320):
    print(show(x), truth(x))


class F(float):
    def __add__(self, other):
        return "F.add"

    def __mul__(self, other):
        return "F.mul"

    def __lt__(self, other):
        return "F.lt"


class I(int):
    def __radd__(self, other):
        return "I.radd"


print(add(F(1.0), 2.0), mul(F(1.0), 2.0), F(1.0) < 2.0, add(2.0, I(3)))


def dot(xs, ys):
    total = 0.0
    for i in range(len(xs)):
        total = total + xs[i] * ys[i]
    return total


def energy(bodies):
    e = 0.0
    for b in bodies:
        x, y, vx, vy, m = b
        e += 0.5 * m * (vx * vx + vy * vy)
        if e != e:
            return "nan"
    return e


xs = [float(i) / 7 for i in range(100)]
ys = [1.0 - float(i) / 11 for i in range(100)]
print(dot(xs, ys), dot([NAN, 1.0], [1.0, 1.0]) != dot([NAN, 1.0], [1.0, 1.0]))
print(energy([[0.0, 0.0, 1.0, 2.0, 3.0], [1.0, 1.0, -0.5, 0.25, 2.0]]))
print(energy([[0.0, 0.0, NAN, 2.0, 3.0]]))
