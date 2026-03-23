"""Purpose: differential coverage for IndexError message parity."""


# 1. List index out of range
try:
    [1, 2, 3][5]
except IndexError as e:
    print(f"IndexError: {e}")

# 2. Negative index out of range
try:
    [1, 2, 3][-4]
except IndexError as e:
    print(f"IndexError: {e}")

# 3. Tuple index out of range
try:
    (1, 2, 3)[5]
except IndexError as e:
    print(f"IndexError: {e}")

# 4. String index out of range
try:
    "abc"[5]
except IndexError as e:
    print(f"IndexError: {e}")

# 5. Empty list index
try:
    [][0]
except IndexError as e:
    print(f"IndexError: {e}")

# 6. Pop from empty list
try:
    [].pop()
except IndexError as e:
    print(f"IndexError: {e}")

# 7. Pop with out-of-range index
try:
    [1, 2].pop(5)
except IndexError as e:
    print(f"IndexError: {e}")

# 8. Range index out of range
try:
    range(5)[10]
except IndexError as e:
    print(f"IndexError: {e}")

# 9. Bytearray index out of range
try:
    bytearray(b"abc")[5]
except IndexError as e:
    print(f"IndexError: {e}")

# 10. Bytes index out of range
try:
    b"abc"[5]
except IndexError as e:
    print(f"IndexError: {e}")

# 11. Assignment to out-of-range list index
try:
    lst = [1, 2, 3]
    lst[10] = 99
except IndexError as e:
    print(f"IndexError: {e}")

# 12. Delete out-of-range list index
try:
    lst = [1, 2]
    del lst[5]
except IndexError as e:
    print(f"IndexError: {e}")
