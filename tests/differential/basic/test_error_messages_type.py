"""Purpose: differential coverage for TypeError message parity."""


# 1. String + int
try:
    "hello" + 42
except TypeError as e:
    print(f"TypeError: {e}")

# 2. Int + string
try:
    42 + "hello"
except TypeError as e:
    print(f"TypeError: {e}")

# 3. Calling a non-callable
try:
    x = 42
    x()
except TypeError as e:
    print(f"TypeError: {e}")

# 4. Wrong number of arguments
def takes_two(a, b):
    pass

try:
    takes_two(1)
except TypeError as e:
    print(f"TypeError: {e}")

# 5. Too many arguments
try:
    takes_two(1, 2, 3)
except TypeError as e:
    print(f"TypeError: {e}")

# 6. Unexpected keyword argument
try:
    takes_two(1, 2, c=3)
except TypeError as e:
    print(f"TypeError: {e}")

# 7. Subscript on int
try:
    x = 42
    x[0]
except TypeError as e:
    print(f"TypeError: {e}")

# 8. Iteration over int
try:
    for _ in 42:
        pass
except TypeError as e:
    print(f"TypeError: {e}")

# 9. Unhasahble type (list as dict key)
try:
    d = {[1, 2]: "value"}
except TypeError as e:
    print(f"TypeError: {e}")

# 10. Comparison between incompatible types
try:
    "hello" < 42
except TypeError as e:
    print(f"TypeError: {e}")

# 11. Unsupported operand types for *
try:
    "hello" * "world"
except TypeError as e:
    print(f"TypeError: {e}")

# 12. Unpack non-iterable
try:
    a, b = 42
except TypeError as e:
    print(f"TypeError: {e}")
