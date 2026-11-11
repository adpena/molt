"""Purpose: integer + and - on boxed operands keep CPython semantics across the
inline-int window (HF-821's late-TIR lane): exact sums at both edges, bools,
big ints, int subclasses, reflected and in-place dunders, and exceptions raised
from a dunder."""

LIMIT = 2**46


def add(a, b):
    return a + b


def sub(a, b):
    return a - b


def iadd(a, b):
    a += b
    return a


def isub(a, b):
    a -= b
    return a


EDGES = [LIMIT - 1, LIMIT, -LIMIT, -LIMIT - 1, 0, 1, -1, 2**63, -(2**63) - 1]
for x in EDGES:
    for y in (1, -1, LIMIT - 1, -LIMIT, x):
        print(add(x, y), sub(x, y), iadd(x, y), isub(x, y))

print(add(True, True), sub(True, 2), add(False, LIMIT - 1), iadd(True, -LIMIT))
print(type(add(True, True)).__name__, type(sub(LIMIT, 1)).__name__)


class Meters(int):
    def __add__(self, other):
        return ("Meters.add", int(self), other)

    def __rsub__(self, other):
        return ("Meters.rsub", other, int(self))


print(add(Meters(3), 4), sub(10, Meters(3)), add(4, Meters(3)))


class Accumulator:
    def __init__(self):
        self.total = 0

    def __iadd__(self, other):
        self.total += other
        return self

    def __radd__(self, other):
        return ("radd", other)


acc = Accumulator()
print(iadd(acc, 5).total, add(7, Accumulator()))


class Refuses:
    def __add__(self, other):
        raise ValueError("no add")

    def __rsub__(self, other):
        raise KeyError("no rsub")


for operation in (add, sub, iadd, isub):
    try:
        print(operation(Refuses(), 1) if operation in (add, iadd) else operation(1, Refuses()))
    except (ValueError, KeyError, TypeError) as error:
        print(operation.__name__, type(error).__name__, error)

try:
    add(1, "x")
except TypeError as error:
    print("TypeError", error)

total = LIMIT - 3
trace = []
for step in range(6):
    total += 1
    trace.append(total)
for step in range(12):
    total -= 1
    trace.append(total)
print(trace[0], trace[5], trace[-1], [type(v).__name__ for v in trace[2:5]])

negative = -LIMIT + 2
for step in range(4):
    negative -= 1
print(negative, negative + 1, negative - LIMIT)


# One boxed operand and one literal: only the boxed word is tag-tested.
def plus_one(x):
    return x + 1


def one_minus(x):
    return 1 - x


def plus_top(x):
    return x + 70368744177663  # LIMIT - 1, the top of the inline window


def minus_bottom(x):
    return x - -70368744177664  # -LIMIT, the bottom of the inline window


def plus_past(x):
    return x + 70368744177664  # LIMIT, past the window


for x in EDGES + [True, False, Meters(3)]:
    for f in (plus_one, one_minus, plus_top, minus_bottom, plus_past):
        value = f(x)
        print(f.__name__, value, type(value).__name__)
for value in ("x", 1.5, None):
    try:
        print(plus_one(value))
    except TypeError as error:
        print("TypeError", error)
