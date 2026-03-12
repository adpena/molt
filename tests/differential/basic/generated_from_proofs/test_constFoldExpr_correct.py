# Generated from Lean theorem: constFoldExpr_correct
# Source: formal/lean/MoltTIR/Passes/ConstFoldCorrect.lean
# Property: Constant folding does not change expression results.
# These expressions should produce identical results whether or not
# the compiler constant-folds them.

# Arithmetic constant expressions
print(2 + 3)
print(10 - 4)
print(6 * 7)
print(10 % 3)

# Nested constant expressions
print((2 + 3) * (4 + 5))
print((10 - 3) * 2 + 1)
print(((1 + 2) + 3) + 4)

# Unary constant expressions
print(-5)
print(-(-5))
print(not True)
print(not False)
print(not not True)

# Mixed with variables (should not be folded but still correct)
x = 10
print(x + 5)
print(x * 2 + 3)
print(x - x)
print(x + 0)
print(x * 1)

# Boolean constant expressions
print(True and True)
print(True and False)
print(False or True)
print(False or False)

# Comparison constant expressions
print(1 == 1)
print(1 == 2)
print(1 < 2)
print(2 < 1)
print(3 < 3)

# Expressions that a constant folder might simplify
print(0 + 42)
print(42 + 0)
print(42 * 1)
print(1 * 42)
print(42 - 0)
print(0 * 999)
