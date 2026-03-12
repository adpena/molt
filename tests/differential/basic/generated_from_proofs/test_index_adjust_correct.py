# Generated from Lean theorem: index_adjust_correct
# Source: formal/lean/MoltTIR/Backend/LuauCorrect.lean
# Property: 0-based index n maps to 1-based index n+1.
# In Python this manifests as correct 0-based indexing on sequences.

# List indexing (0-based)
lst = [10, 20, 30, 40, 50]
for i in range(len(lst)):
    print(f"lst[{i}] = {lst[i]}")

# Negative indexing
print(lst[-1])
print(lst[-2])
print(lst[-len(lst)])

# Tuple indexing
tup = (100, 200, 300)
for i in range(len(tup)):
    print(f"tup[{i}] = {tup[i]}")

# String indexing
s = "hello"
for i in range(len(s)):
    print(f"s[{i}] = {s[i]}")

# Boundary: empty sequence
empty: list[int] = []
print(len(empty))
try:
    print(empty[0])
except IndexError as e:
    print(f"IndexError: {e}")

# Boundary: single element
single = [99]
print(single[0])
print(single[-1])

# Index out of range
try:
    print(lst[5])
except IndexError as e:
    print(f"IndexError: {e}")

try:
    print(lst[-6])
except IndexError as e:
    print(f"IndexError: {e}")

# Verify index arithmetic: element at i equals element at i - len for negative
for i in range(len(lst)):
    print(lst[i] == lst[i - len(lst)])
