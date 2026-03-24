"""Purpose: differential coverage for arithmetic error message parity."""


# 1. Integer division by zero
try:
    1 // 0
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# 2. Float division by zero
try:
    1.0 / 0.0
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# 3. Modulo by zero (int)
try:
    5 % 0
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# 4. Modulo by zero (float)
try:
    5.0 % 0.0
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# 5. True division by zero (int)
try:
    1 / 0
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# 6. divmod by zero
try:
    divmod(10, 0)
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# 7. pow with non-int modulus
try:
    pow(2, 3, 1.5)
except TypeError as e:
    print(f"TypeError: {e}")

# 8. int too large to convert to float
try:
    float(10 ** 400)
except OverflowError as e:
    print(f"OverflowError: {e}")

# 9. Negative shift count
try:
    1 << -1
except ValueError as e:
    print(f"ValueError: {e}")

# 10. Zero to negative power (float)
try:
    0.0 ** -1
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# 11. complex modulo
try:
    complex(1, 2) % complex(1, 0)
except TypeError as e:
    print(f"TypeError: {e}")

# 12. round with non-integer ndigits type
try:
    round(3.14, "2")
except TypeError as e:
    print(f"TypeError: {e}")
