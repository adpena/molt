"""Purpose: differential coverage for ValueError message parity."""


# 1. Invalid literal for int
try:
    int("abc")
except ValueError as e:
    print(f"ValueError: {e}")

# 2. Invalid literal for float
try:
    float("not-a-number")
except ValueError as e:
    print(f"ValueError: {e}")

# 3. Too many values to unpack
try:
    a, b = [1, 2, 3]
except ValueError as e:
    print(f"ValueError: {e}")

# 4. Not enough values to unpack
try:
    a, b, c = [1, 2]
except ValueError as e:
    print(f"ValueError: {e}")

# 5. List.remove with missing value
try:
    [1, 2, 3].remove(99)
except ValueError as e:
    print(f"ValueError: {e}")

# 6. Invalid base for int()
try:
    int("ff", 1)
except ValueError as e:
    print(f"ValueError: {e}")

# 7. math.sqrt of negative (via ** 0.5 won't raise, use int conversion)
try:
    int("", 10)
except ValueError as e:
    print(f"ValueError: {e}")

# 8. chr out of range
try:
    chr(0x110000)
except ValueError as e:
    print(f"ValueError: {e}")

# 9. Negative value in bytes
try:
    bytes([-1])
except ValueError as e:
    print(f"ValueError: {e}")

# 10. Too large value in bytes
try:
    bytes([256])
except ValueError as e:
    print(f"ValueError: {e}")
