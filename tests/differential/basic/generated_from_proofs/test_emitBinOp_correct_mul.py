# Generated from Lean theorem: emitBinOp_correct_mul
# Source: formal/lean/MoltTIR/Backend/LuauCorrect.lean
# Property: a * b on integers produces the same result in Molt as in Python.

# Basic cases
print(0 * 0)
print(1 * 1)
print(2 * 3)
print(-2 * 3)
print(-2 * -3)

# Identity and zero
print(42 * 1)
print(1 * 42)
print(42 * 0)
print(0 * 42)

# Negation via multiplication
print(5 * -1)
print(-1 * 5)

# Large integers
print(10**9 * 10**9)
print(-(10**9) * 10**9)

# Commutativity
a, b = 17, 53
print(a * b)
print(b * a)
print(a * b == b * a)

# Associativity
a, b, c = 3, 7, 11
print((a * b) * c)
print(a * (b * c))
print((a * b) * c == a * (b * c))

# Distributivity
a, b, c = 5, 3, 7
print(a * (b + c))
print(a * b + a * c)
print(a * (b + c) == a * b + a * c)
