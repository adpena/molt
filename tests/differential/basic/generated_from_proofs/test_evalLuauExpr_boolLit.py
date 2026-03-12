# Generated from Lean theorem: evalLuauExpr_boolLit
# Source: formal/lean/MoltTIR/Backend/LuauSemantics.lean
# Property: Boolean literals evaluate to their expected values.

print(True)
print(False)
print(type(True).__name__)
print(type(False).__name__)

# Bool is a subclass of int
print(isinstance(True, int))
print(isinstance(False, int))
print(True == 1)
print(False == 0)
print(True + True)
print(False + False)
