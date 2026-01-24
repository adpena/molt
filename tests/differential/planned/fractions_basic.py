"""Purpose: differential coverage for fractions basic."""

import fractions


f = fractions.Fraction("1/3") + fractions.Fraction(1, 6)
print(f)
print(f.numerator, f.denominator)
