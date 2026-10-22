from fractions import Fraction
import struct
import sys
import math
import numbers

# Complex arithmetic
a = Fraction(7, 12)
b = Fraction(5, 8)
print(a + b)
print(a**2)
print(abs(Fraction(-3, 4)))

# Comparison
print(Fraction(1, 3) < Fraction(1, 2))
print(Fraction(2, 4) == Fraction(1, 2))

# Mixed with int/float
print(Fraction(1, 3) + 1)
print(Fraction(1, 4) * 4)

# One rounding of the exact ratio, including underflow and exponent transitions.


def bits(value):
    return struct.pack(">d", value).hex()


ratio_cases = [
    ("min_subnormal", 1, 2**1074),
    ("half_to_zero", 1, 2**1075),
    ("above_half", 3, 2**1076),
    ("subnormal_even_up", 3, 2**1075),
    ("subnormal_even_down", 5, 2**1075),
    ("largest_subnormal", 2**52 - 1, 2**1074),
    ("min_normal_tie", 2**53 - 1, 2**1075),
    ("below_min_normal_tie", 2**54 - 3, 2**1076),
    ("normal_even_down", 2**53 + 1, 2**53),
    ("normal_even_up", 2**53 + 3, 2**53),
    ("balanced_huge", 2**2000 + 1, 2**2000 - 1),
    ("heap_i64_double_round", 2**53 + 1, 3),
    ("heap_i64_inexact", 2**60 + 4, 3),
    ("heap_i64_exact", 2**60, 3),
    ("i64_max", 2**63 - 1, 3),
    ("below_overflow_tie", (2**54 - 1) * 2**970 - 1, 1),
    ("overflow_tie", (2**54 - 1) * 2**970, 1),
]
for name, numerator, denominator in ratio_cases:
    for sign in (1, -1):
        n = numerator * sign
        try:
            quotient = bits(n / denominator)
        except OverflowError as error:
            quotient = type(error).__name__ + ": " + str(error)
        try:
            fraction = bits(float(Fraction(n, denominator)))
        except OverflowError as error:
            fraction = type(error).__name__ + ": " + str(error)
        print(name, sign, quotient, fraction)

# Integer division preserves the denominator's sign at zero; Fraction normalizes it.
print("zero_sign", bits(0 / -(2**2000)), bits(float(Fraction(0, -(2**2000)))))
print("int_truncation", [int(Fraction(n, 3)) for n in (-7, -1, 0, 1, 7)])


class DerivedFraction(Fraction):
    pass


print("subclass", int(DerivedFraction(-7, 3)), bits(float(DerivedFraction(1, 2**1074))))


class DerivedInt(int):
    pass


print("integer_subclass", bits(DerivedInt(2**53 + 1) / 3))


print(
    "numeric_tower",
    [
        isinstance(Fraction(1, 2), kind)
        for kind in (
            numbers.Number,
            numbers.Complex,
            numbers.Real,
            numbers.Rational,
            numbers.Integral,
        )
    ],
)
print("concrete_rational", not Fraction.__abstractmethods__)
for value in [Fraction(n, 2) for n in (-7, -5, -1, 1, 5, 7)] + [
    Fraction(2**200 + 1, 3)
]:
    print(
        "rounding",
        str(value),
        int(value),
        math.trunc(value),
        math.floor(value),
        math.ceil(value),
        round(value),
    )
for value in (
    Fraction(125, 100),
    Fraction(-125, 100),
    Fraction(125, 1),
    Fraction(-125, 1),
):
    print("digits", [str(round(value, n)) for n in (-2, -1, 0, 1, 2, True)])
print(
    "round_subclass",
    type(round(DerivedFraction(7, 2), 1)).__name__,
    type(round(DerivedFraction(7, 2))).__name__,
)


class IntOverrideFraction(Fraction):
    def __int__(self):
        return 99


print(
    "trunc_not_int_override",
    int(IntOverrideFraction(-7, 3)),
    math.trunc(IntOverrideFraction(-7, 3)),
)


class IndexDigits:
    def __index__(self):
        return 2


try:
    round(Fraction(1, 3), IndexDigits())
except TypeError as error:
    print("digits_protocol", type(error).__name__, str(error))

# Mixed-domain arithmetic must perform the builtin operation after promotion,
# rather than construct a rational from the float and retain Fraction type.
for value in (
    Fraction(3, 2) + 0.25,
    0.25 + Fraction(3, 2),
    Fraction(3, 2) - 0.25,
    0.25 - Fraction(3, 2),
    Fraction(3, 2) * 0.25,
    0.25 * Fraction(3, 2),
    Fraction(3, 2) / 0.25,
    0.25 / Fraction(3, 2),
    Fraction(3, 2) // 0.25,
    0.25 // Fraction(3, 2),
    Fraction(3, 2) % 0.25,
    0.25 % Fraction(3, 2),
    Fraction(3, 2) // Fraction(1, 3),
    2 // Fraction(3, 2),
    Fraction(3, 2) // 2,
):
    print("mixed_arithmetic", type(value).__name__, str(value))

f = Fraction(3, 2)
print(
    "constructor_only_string",
    [
        method("2") is NotImplemented
        for method in (
            f.__add__,
            f.__radd__,
            f.__sub__,
            f.__rsub__,
            f.__mul__,
            f.__rmul__,
            f.__truediv__,
            f.__rtruediv__,
            f.__floordiv__,
            f.__rfloordiv__,
            f.__mod__,
            f.__rmod__,
        )
    ],
)


class ReflectedOperand:
    def __radd__(self, other):
        return "reflected add"

    def __rsub__(self, other):
        return "reflected sub"

    def __rtruediv__(self, other):
        return "reflected div"


print(
    "cooperation",
    f + ReflectedOperand(),
    f - ReflectedOperand(),
    f / ReflectedOperand(),
)


class ReflectedFloat(float):
    def __rsub__(self, other):
        return type(other).__name__


print("float_order", f - ReflectedFloat(0.25), ReflectedFloat(0.25) - f)
for value in (
    Fraction(2, 3) ** -3,
    Fraction(-2, 3) ** -3,
    Fraction(4, 1) ** Fraction(1, 2),
    Fraction(4, 1) ** 0.5,
    4 ** Fraction(1, 2),
    2 ** Fraction(3, 1),
    Fraction(-1, 1) ** (2**64 + 1),
    Fraction(1, 1) ** -(2**64 + 1),
    DerivedFraction(2, 3) ** 2,
):
    print("power_protocol", type(value).__name__, str(value))
try:
    Fraction(0) ** -1
except ZeroDivisionError as error:
    print("power_zero", str(error))


for method in (Fraction(2).__pow__, Fraction(2).__rpow__):
    try:
        method(3, None)
    except TypeError:
        accepts_modulo = False
    else:
        accepts_modulo = True
    print("power_signature", accepts_modulo == (sys.version_info >= (3, 14)))
