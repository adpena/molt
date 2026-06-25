# Luau numbers are f64. CheckedMul on the Luau backend must flag products whose
# integer exactness cannot be proven around 2^53, forcing the boxed slow loop.

values = [
    (3, (1 << 53) - 1),
    (3, 1 << 53),
    (3, (1 << 53) + 1),
    ((1 << 26) + 1, (1 << 27) + 1),
]

for lhs, rhs in values:
    product = lhs * rhs
    print(lhs, rhs, product, product - lhs * (rhs - 1))
