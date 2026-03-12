# Generated from Lean theorem: emitExpr_correct_val
# Source: formal/lean/MoltTIR/Backend/LuauCorrect.lean
# Property: Value literals in expressions evaluate correctly.

# Integer values in expressions
print(1 + 0)
print(0 + 0)
print(42 + 0)

# Boolean values in expressions
print(True and True)
print(False or False)

# String values in expressions
print("hello" + "")
print("" + "world")

# None in expressions
print(None is None)
print(None == None)
