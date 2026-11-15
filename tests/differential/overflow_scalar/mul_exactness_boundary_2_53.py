# Integer products around 2^53, where an f64 loses exactness: a checked multiply
# must keep every product exact, as CPython's arbitrary-precision int does.

values = [
    (3, (1 << 53) - 1),
    (3, 1 << 53),
    (3, (1 << 53) + 1),
    ((1 << 26) + 1, (1 << 27) + 1),
]

for lhs, rhs in values:
    product = lhs * rhs
    print(lhs, rhs, product, product - lhs * (rhs - 1))
