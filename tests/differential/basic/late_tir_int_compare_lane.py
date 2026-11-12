"""Purpose: integer comparisons on boxed operands keep CPython semantics across
the inline-int window (HF-821's late-TIR lane): ints at both edges, big ints,
bools, floats, mixed types, literals, rich-comparison dunders on int
subclasses and other classes, NotImplemented fallbacks, and raised errors."""

LIMIT = 2**46


def lt(a, b):
    return a < b


def le(a, b):
    return a <= b


def gt(a, b):
    return a > b


def ge(a, b):
    return a >= b


def eq(a, b):
    return a == b


def ne(a, b):
    return a != b


OPS = (lt, le, gt, ge, eq, ne)
EDGES = [LIMIT - 1, LIMIT, -LIMIT, -LIMIT - 1, 0, 1, -1, 2**63, -(2**63) - 1, 2**100]
for x in EDGES:
    row = []
    for y in (LIMIT - 1, -LIMIT, 0, x, x + 1, x - 1):
        row.append("".join("1" if f(x, y) else "0" for f in OPS))
    print(x, " ".join(row))

# Bools and floats are not inline ints: they take the runtime entry.
for x, y in ((True, 1), (False, 0), (True, 2), (1, 1.0), (LIMIT - 1, float(LIMIT - 1)),
             (2**53 + 1, float(2**53)), (1, float("nan")), (0, -0.0), (3, 2.5)):
    print(x, y, [f(x, y) for f in OPS], [f(y, x) for f in OPS])

print(type(lt(1, 2)).__name__, lt(1, 2) is True, eq(5, 5) is True, ne(5, 5) is False)


def below_two(n):
    return n < 2


def is_zero(n):
    return n == 0


def at_least_top(n):
    return n >= 70368744177663  # LIMIT - 1, the top of the inline window


for n in (-LIMIT, -1, 0, 1, 2, 3, LIMIT - 1, LIMIT, 2**70, True, False, 1.5):
    print(n, below_two(n), is_zero(n), at_least_top(n))


class Loud(int):
    def __lt__(self, other):
        return ("Loud.lt", int(self), other)

    def __eq__(self, other):
        return ("Loud.eq", int(self), other)

    __hash__ = int.__hash__


print(lt(Loud(3), 4), lt(4, Loud(3)), eq(Loud(3), 3), eq(3, Loud(3)), gt(4, Loud(3)))


class Version:
    def __init__(self, major):
        self.major = major

    def __lt__(self, other):
        if isinstance(other, int):
            return self.major < other
        return NotImplemented

    def __gt__(self, other):
        if isinstance(other, int):
            return self.major > other
        return NotImplemented


print(lt(Version(2), 3), lt(3, Version(2)), gt(3, Version(2)), eq(Version(2), 2))

for f in (lt, le, gt, ge):
    try:
        print(f(1, "x"))
    except TypeError as error:
        print(f.__name__, "TypeError", error)
print(eq(1, "1"), ne(1, None), eq(None, 0))


class Raises:
    def __lt__(self, other):
        raise ValueError("no order")

    def __gt__(self, other):
        raise KeyError("no reverse order")


for args in ((Raises(), 1), (1, Raises())):
    try:
        print(lt(*args))
    except (ValueError, KeyError) as error:
        print("raised", type(error).__name__, error)

count = 0
i = LIMIT - 4
while i < LIMIT + 2:
    if i >= LIMIT - 1 and i != LIMIT:
        count += 1
    i += 1
print(count, i, 0 < i < 2**47, -1 < 0 <= 0 < 1)


# Comparisons inside try regions: the check after each comparison keeps the
# comparison's exception source, on the fast arm, the literal arm and the call.
def guarded(x, y):
    hits = []
    try:
        if x < 2:
            hits.append("lt-literal")
        if x == y:
            hits.append("eq")
        if y >= x:
            hits.append("ge")
        if x != 0 and y > -1:
            hits.append("both")
    except (TypeError, ValueError, KeyError) as error:
        hits.append(type(error).__name__)
    finally:
        hits.append("finally")
    return hits


for x, y in ((1, 1), (5, 3), (LIMIT, LIMIT - 1), (1, "x"), (Raises(), 1), (2.5, 2), (True, 1)):
    print(guarded(x, y))
