# Generated from Lean theorem: constFoldInstr_correct
# Source: formal/lean/MoltTIR/Passes/ConstFoldCorrect.lean
# Property: Instruction-level constant folding preserves semantics.
# Variable assignments with foldable RHS should produce the same values.

# Simple foldable assignments
a = 2 + 3
print(a)
b = 10 * 5
print(b)
c = 100 - 1
print(c)
d = 17 % 5
print(d)

# Nested foldable
e = (2 + 3) * (4 + 5)
print(e)

# Unary foldable
f = -42
print(f)
g = not True
print(g)

# Use folded results in subsequent operations
h = a + b
print(h)
i = a * b - c
print(i)
