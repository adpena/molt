"""Purpose: differential coverage for float ops."""

print(float(), float(1), float(True))
print(float(" 2.5 "), float(b"3.5"))

print(1.0 + 2.5, 5.0 - 2.0, 2.0 * 3.5)
print(5 / 2, 5 // 2, 5 % 2)
print(5.0 // 2.0, 5.0 % 2.0)
print(2**3, 2.0**3, 2**3.0, 2.0**3.0)
print(1.0 == 1, 1.0 == 2, 1.0 < 2, 2.0 <= 2, 3.0 > 2, 3.0 >= 4)

nan = float("nan")
print(nan == nan, nan < 1, nan > 1)
print(float("inf"), float("-inf"))

# Exact integer-ratio rounding must not cast each operand to float first.
print("integer-ratio", 9007199254740993 / 3, -9007199254740993 / 3)


def dynamic_integer_ratio(left, right):
    return left / right


print("dynamic-integer-ratio", dynamic_integer_ratio(9007199254740993, 3))

# Floor division and modulo are one corrected CPython divmod primitive, not
# independent host floor/remainder expressions. The signed-zero cases matter.
for left, right in [(1.0, 0.1), (-1.0, 0.1), (-0.0, 3.0), (0.0, -3.0)]:
    print("float-divmod", left, right, left // right, left % right, divmod(left, right))
print("selected-zero", min(-0.0, 0.0), min(0.0, -0.0), max(-0.0, 0.0), max(0.0, -0.0))
