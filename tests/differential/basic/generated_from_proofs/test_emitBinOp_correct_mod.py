# Generated from Lean theorem: emitBinOp_correct_mod
# Source: formal/lean/MoltTIR/Backend/LuauCorrect.lean
# Property: a % b on integers produces the same result in Molt as in Python.

# Basic cases
print(10 % 3)
print(10 % 5)
print(0 % 1)
print(0 % 7)
print(1 % 1)

# Negative dividend (Python uses floored division for mod)
print(-10 % 3)
print(-1 % 3)
print(-7 % 4)

# Negative divisor
print(10 % -3)
print(1 % -3)
print(7 % -4)

# Both negative
print(-10 % -3)
print(-7 % -4)

# Large values
print(10**18 % 7)
print(-(10**18) % 7)
print(10**18 % (10**9 + 7))

# Division by zero
try:
    print(1 % 0)
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

# Invariant: a == (a // b) * b + (a % b)
for a in [10, -10, 7, -7, 0, 100]:
    for b in [3, -3, 7, -7, 1, -1]:
        print(a == (a // b) * b + (a % b))
