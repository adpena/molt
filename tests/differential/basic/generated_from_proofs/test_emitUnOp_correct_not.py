# Generated from Lean theorem: emitUnOp_correct_not
# Source: formal/lean/MoltTIR/Backend/LuauCorrect.lean
# Property: not b on booleans produces correct result.

# Truth table
print(not True)
print(not False)

# Double negation
print(not not True)
print(not not False)
print(not not True == True)
print(not not False == False)

# Involution
for b in [True, False]:
    print(not not b == b)

# De Morgan's laws
for a in [True, False]:
    for b in [True, False]:
        print(not (a and b) == (not a or not b))
        print(not (a or b) == (not a and not b))

# not on truthy/falsy integer values
print(not 0)
print(not 1)
print(not -1)
print(not 42)
