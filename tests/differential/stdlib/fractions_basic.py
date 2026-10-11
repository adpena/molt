"""Purpose: differential coverage for fractions basic."""

import fractions


f = fractions.Fraction("1/3") + fractions.Fraction(1, 6)
print(f)
print(f.numerator, f.denominator)

import sys

old_digits = sys.get_int_max_str_digits()
try:
    sys.set_int_max_str_digits(640)
    allowed = 10**639
    rejected = 10**640
    print("fraction boundary", len(str(fractions.Fraction(allowed, 3))))
    for label, value in [
        ("numerator", fractions.Fraction(rejected, 3)),
        ("denominator", fractions.Fraction(3, rejected)),
        ("integer", fractions.Fraction(rejected)),
    ]:
        for render in [str, repr]:
            try:
                render(value)
                print(label, render.__name__, "accepted")
            except ValueError as exc:
                print(label, render.__name__, type(exc).__name__, str(exc))
    sys.set_int_max_str_digits(0)
    print("fraction disabled", len(str(fractions.Fraction(rejected))))
finally:
    sys.set_int_max_str_digits(old_digits)
