# Generated from Lean theorem: emitBinOp_correct_lt
# Source: formal/lean/MoltTIR/Backend/LuauCorrect.lean
# Property: a < b on integers produces correct boolean result.

# Basic cases
print(0 < 1)
print(1 < 0)
print(0 < 0)
print(-1 < 0)
print(0 < -1)
print(-1 < 1)
print(1 < -1)

# Irreflexivity
for v in [0, 1, -1, 42, -42]:
    print(v < v)

# Transitivity
print(1 < 2 and 2 < 3)
print(1 < 3)

# Asymmetry
a, b = 3, 7
print(a < b)
print(b < a)
print(not (a < b and b < a))

# Large integers
print(10**18 < 10**18 + 1)
print(10**18 + 1 < 10**18)
print(-(10**18) < 10**18)
print(10**18 < -(10**18))

# Boundary values
print(2**31 - 1 < 2**31)
print(2**63 - 1 < 2**63)
print(-(2**31) < 2**31 - 1)

# Trichotomy: exactly one of a<b, a==b, a>b
for a in [-5, 0, 5, 42]:
    for b in [-5, 0, 5, 42]:
        lt = a < b
        eq = a == b
        gt = a > b
        print(int(lt) + int(eq) + int(gt) == 1)
