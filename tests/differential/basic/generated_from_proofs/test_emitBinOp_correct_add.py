# Generated from Lean theorem: emitBinOp_correct_add
# Source: formal/lean/MoltTIR/Backend/LuauCorrect.lean
# Property: a + b on integers produces the same result in Molt as in Python.

# Basic cases
print(0 + 0)
print(1 + 2)
print(-1 + 1)
print(-1 + (-1))

# Identity
print(42 + 0)
print(0 + 42)

# Large integers
print(10**18 + 10**18)
print(-(10**18) + 10**18)

# Boundary values
print(2**31 - 1 + 1)
print(-(2**31) + (-1))
print(2**63 - 1 + 1)
print(-(2**63) + (-1))

# Commutativity (verified by printing both orders)
a, b = 17, 53
print(a + b)
print(b + a)
print(a + b == b + a)

# Associativity
a, b, c = 11, 22, 33
print((a + b) + c)
print(a + (b + c))
print((a + b) + c == a + (b + c))
