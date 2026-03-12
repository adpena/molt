# Generated from Lean theorem: emitExpr_correct
# Source: formal/lean/MoltTIR/Backend/LuauCorrect.lean
# Property: Full structural induction -- all expression forms evaluate correctly.

# Literal expressions
print(42)
print(True)
print("hello")
print(None)

# Variable reference expressions
x = 10
y = 20
print(x)
print(y)

# Binary expressions (nested)
print(x + y)
print(x - y)
print(x * y)
print((x + y) * (x - y))
print(x + y + 1)

# Unary expressions
print(-x)
print(not True)
print(-(-x))

# Mixed nesting
a = 3
b = 4
c = 5
print(a + b * c)
print((a + b) * c)
print(-(a + b))
print(a * b + c * (a - b))

# Deeply nested
print(((1 + 2) * 3 + 4) * 5)
print(-(((1 + 2) * 3)))
