# Generated from Lean theorem: emitBinOp_correct_sub
# Source: formal/lean/MoltTIR/Backend/LuauCorrect.lean
# Property: a - b on integers produces the same result in Molt as in Python.

# Basic cases
print(0 - 0)
print(3 - 1)
print(1 - 3)
print(-1 - (-1))

# Identity
print(42 - 0)

# Self-subtraction
print(99 - 99)
print(-99 - (-99))

# Large integers
print(10**18 - 1)
print(1 - 10**18)
print(-(10**18) - 10**18)

# Boundary values
print(2**31 - 1)
print(-(2**31) - 1)
print(2**63 - 1)
print(-(2**63) - 1)

# Non-commutativity
a, b = 17, 53
print(a - b)
print(b - a)
print(a - b == -(b - a))
