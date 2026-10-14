"""Purpose: in-place int arithmetic on constants keeps Python's unbounded results.

Guards the SimpleIR constant folders, which fold with i64 wrapping (HF-214).
"""

def square_in_place():
    x = 3037000500          # inside the 47-bit inline range
    x *= x                  # 9223372037000250000 > i64::MAX
    return x


def square_to_zero():
    y = 1099511627776       # 2**40
    y *= y                  # 2**80; wraps to 0 in i64
    return y


def sum_in_place():
    z = -70368744177664     # inline minimum
    z -= 70368744177664     # stays in i64, leaves the 47-bit range
    return z


print(square_in_place())
print(square_to_zero())
print(sum_in_place())
