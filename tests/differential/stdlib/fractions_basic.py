from fractions import Fraction

a = Fraction(1, 3)
b = Fraction(2, 5)
print(a + b)
print(a * b)
print(a - b)
print(a / b)
print(Fraction(0.1))  # from float
print(Fraction('3/7'))  # from string
print(float(a))
print(a.limit_denominator(10))
