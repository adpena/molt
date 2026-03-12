# Generated from Lean theorem: evalLuauExpr_intLit
# Source: formal/lean/MoltTIR/Backend/LuauSemantics.lean
# Property: Integer literals evaluate to their expected values.

print(0)
print(1)
print(-1)
print(42)
print(-42)
print(2**31 - 1)
print(2**31)
print(-(2**31))
print(2**63 - 1)
print(2**63)
print(-(2**63))
print(10**18)
print(-(10**18))

# Verify type
print(type(0).__name__)
print(type(42).__name__)
print(type(-1).__name__)
print(type(10**18).__name__)
