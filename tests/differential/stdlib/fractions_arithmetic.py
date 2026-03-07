from fractions import Fraction

# Complex arithmetic
a = Fraction(7, 12)
b = Fraction(5, 8)
print(a + b)
print(a ** 2)
print(abs(Fraction(-3, 4)))

# Comparison
print(Fraction(1, 3) < Fraction(1, 2))
print(Fraction(2, 4) == Fraction(1, 2))

# Mixed with int/float
print(Fraction(1, 3) + 1)
print(Fraction(1, 4) * 4)
