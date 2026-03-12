# Generated from Lean theorem: emitBinOp_correct_eq
# Source: formal/lean/MoltTIR/Backend/LuauCorrect.lean
# Property: a == b on integers produces correct boolean result.

# Basic cases
print(0 == 0)
print(1 == 1)
print(1 == 2)
print(-1 == -1)
print(-1 == 1)

# Identity
x = 42
print(x == x)
print(x == 42)

# Large integers
print(10**18 == 10**18)
print(10**18 == 10**18 + 1)
print(-(10**18) == -(10**18))

# Boundary values
print(2**31 - 1 == 2**31 - 1)
print(2**31 == 2**31)
print(2**63 - 1 == 2**63 - 1)
print(2**63 == 2**63)

# Reflexivity
for v in [0, 1, -1, 42, -42, 10**18, -(10**18)]:
    print(v == v)

# Symmetry
a, b = 17, 53
print((a == b) == (b == a))
a2, b2 = 42, 42
print((a2 == b2) == (b2 == a2))

# Negation
print(not (1 == 2))
print(not (1 == 1))
