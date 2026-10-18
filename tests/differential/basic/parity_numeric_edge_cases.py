# Parity test: numeric edge cases
# All output via print() for diff comparison

print("=== Large integers (>64 bit) ===")
print(2**200)
print(10**50)
print(2**100 + 2**100)
print(2**100 % 17)

print("=== Very large int operations ===")
a = 10**30
b = 10**30 + 1
print(a < b)
print(b - a)
print(a * a == 10**60)

print("=== Int/float boundary ===")
print(2**53)
print(2**53 + 1)
print(float(2**53))
print(float(2**53 + 1))
print(float(2**53) == float(2**53 + 1))
print(2**53 == 2**53 + 1)

print("=== Float special values ===")
inf = float("inf")
ninf = float("-inf")
nan = float("nan")
print(repr(inf))
print(repr(ninf))
print(repr(nan))

print("=== inf arithmetic ===")
print(inf + inf)
print(inf - inf)
print(inf * -1)
print(1 / inf)

print("=== nan behavior ===")
print(nan == nan)
print(nan != nan)
print(nan < 0)
print(nan > 0)
print(nan == 0)

print("=== nan in collections ===")
print(nan in [1.0, nan, 3.0])  # identity match
print(float("nan") in [1.0, float("nan"), 3.0])  # no identity match

print("=== Negative zero ===")
pz = 0.0
nz = -0.0
print(pz == nz)
print(repr(pz))
print(repr(nz))
import math

print(math.copysign(1.0, pz))
print(math.copysign(1.0, nz))

try:
    1 / pz
except ZeroDivisionError:
    print("ZeroDivisionError for 1/0.0")

try:
    1 / nz
except ZeroDivisionError:
    print("ZeroDivisionError for 1/-0.0")

print("=== float precision ===")
print(0.1 + 0.2)
print(0.1 + 0.2 == 0.3)
print(repr(0.1 + 0.2))

print("=== int/float coercion ===")
print(type(1 + 1.0).__name__)
print(type(6 / 2).__name__)
print(type(6 // 2).__name__)
print(type(6 // 2.0).__name__)
print(type(True + 1).__name__)
print(type(True + 1.0).__name__)

print("=== bool is int ===")
print(True + True)
print(True * 10)
print(True == 1)
print(False == 0)
print(isinstance(True, int))
print(float(True))
print(float(False))

print("=== divmod edge cases ===")
print(divmod(-10, 3))
print(divmod(10, -3))

print("=== divmod with floats ===")
print(divmod(10.0, 3.0))

print("=== Banker's rounding (round half to even) ===")
print(round(0.5))
print(round(1.5))
print(round(2.5))
print(round(3.5))

print("=== round returns int when ndigits is None ===")
print(type(round(2.5)).__name__)
print(type(round(2.5, 0)).__name__)
print(type(round(2)).__name__)

print("=== complex numbers ===")
c1 = 3 + 4j
c2 = 1 - 2j
print(c1 + c2)
print(c1 * c2)
print(c1.real)
print(c1.imag)
print(abs(c1))

print("=== complex edge cases ===")
print(1j * 1j)
print((1 + 0j) == 1)
print(type(1 + 0j).__name__)

print("=== int conversions ===")
print(int(3.7))
print(int(-3.7))
print(int("0xff", 16))
print(int("0b1010", 2))
print(int("0o17", 8))

print("=== float conversions ===")
print(float("3.14"))
print(float("nan") != float("nan"))
print(float("1e10"))
print(float(".5"))

print("=== pow with three args ===")
print(pow(2, 10, 1000))
print(pow(3, 100, 97))
print(pow(7, 0, 13))
