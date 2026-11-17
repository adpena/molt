"""Type flow (design 78 section 4): exact int contexts keep Python int semantics.

Every function here is called only with exact ints from its call sites, so a
whole-program type flow may give it an int context. The results must still
follow CPython: a subclass never reaches an exact context, bool keeps its own
type, floor division and modulo round toward negative infinity, and + - *,
negation and abs overflow into big ints. Mixed int/float and true division
produce float.
"""


class MyInt(int):
    def __add__(self, other):
        return "MyInt.__add__"


def add(a, b):
    return a + b


def sub(a, b):
    return a - b


def mul(a, b):
    return a * b


def floordiv(a, b):
    return a // b


def mod(a, b):
    return a % b


def neg(a):
    return -a


def absolute(a):
    return abs(a)


def truediv(a, b):
    return a / b


def annotated_add(a: int, b: int) -> int:
    return a + b


big = 2**62
print(add(3, 4), type(add(3, 4)).__name__)
print(add(big, big), add(big, big) == 2**63)
print(sub(-big, big), sub(-(2**63), 1))
print(mul(big, 4), mul(-3, 2**61))
print(neg(-(2**63)), type(neg(-(2**63))).__name__)
print(absolute(-(2**63)), absolute(-5))
for a, b in [(7, 2), (-7, 2), (7, -2), (-7, -2), (-(2**63), -1)]:
    print(a, b, floordiv(a, b), mod(a, b))

# bool is a subclass of int: it computes as int and keeps its own identity.
print(add(True, 1), type(add(True, 1)).__name__)
print(add(True, True), type(neg(True)).__name__)
print(floordiv(True, True), mod(False, True))

# A subclass instance never takes the exact int path.
print(add(MyInt(1), 2))
print(annotated_add(MyInt(5), 1), annotated_add("a", "b"))

# Mixed int/float and true division are float.
print(add(1, 2.5), type(add(1, 2.5)).__name__)
print(truediv(7, 2), truediv(-7, 2), type(truediv(4, 2)).__name__)
print(mul(3, 0.5), sub(2**53 + 1, 0.0))
