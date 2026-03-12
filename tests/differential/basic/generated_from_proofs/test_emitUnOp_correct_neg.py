# Generated from Lean theorem: emitUnOp_correct_neg
# Source: formal/lean/MoltTIR/Backend/LuauCorrect.lean
# Property: -a on integers produces correct result.

# Basic cases
print(-0)
print(-1)
print(-(-1))
print(-42)
print(-(-42))

# Double negation is identity
for v in [0, 1, -1, 42, -42, 10**18, -(10**18)]:
    print(-(-v) == v)

# Negation inverts sign
print(-5 < 0)
print(-(-5) > 0)
print(-0 == 0)

# Large integers
print(-(10**18))
print(-(-(10**18)))
print(-(2**63))
print(-(-(2**63)))

# Negation and addition: a + (-a) == 0
for v in [1, -1, 42, -42, 10**18, -(10**18)]:
    print(v + (-v) == 0)

# Negation distributes over addition
a, b = 17, 53
print(-(a + b) == (-a) + (-b))
