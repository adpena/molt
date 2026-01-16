def section(name):
    print(f"--- {name} ---")


section("Modulo with negative numbers")
# Python's % matches the sign of the divisor, C matches dividend
print(10 % 3)
print(-10 % 3)
print(10 % -3)
print(-10 % -3)

section("Float special values")
inf = float("inf")
nan = float("nan")

print(inf + 1)
print(inf - inf)  # nan
# Nan comparisons are tricky, usually not equal to itself
# But we print the representation to check identity/value logic
print(repr(nan))
print(nan == nan)
print(nan != nan)
print(inf > 9e99)

section("Pow 3-arg")
print(pow(2, 3, 5))
print(pow(10, 2, 3))
# Negative base with fractional exponent -> complex (maybe unsupported or error)
try:
    print((-1) ** 0.5)
except ValueError:
    print("ValueError caught (expected for float domain)")
except TypeError:
    print("TypeError caught")
except Exception as e:
    print(f"{type(e).__name__} caught")

section("Division by zero")
try:
    print(1 / 0)
except ZeroDivisionError:
    print("ZeroDivisionError caught (float)")

try:
    print(1 // 0)
except ZeroDivisionError:
    print("ZeroDivisionError caught (int)")

section("Large integers")
# Check promotion boundaries if implemented, or at least lack of overflow crash
a = 2**63 - 1
b = 10
print(a + b)
print((a + b) * 2)
