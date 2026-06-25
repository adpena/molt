# Signed CheckedMul must detect both positive and negative i64 overflow.
# The wrapped product is never observable as a Python int.

pairs = [
    (-(1 << 62), 2),       # exactly i64::MIN, still representable
    (-(1 << 62) - 1, 2),   # below i64::MIN, bigint
    (-(1 << 31), 1 << 32), # exactly i64::MIN
    (-(1 << 31) - 1, 1 << 32),
    (-(1 << 63), -1),      # over i64::MAX
]

for lhs, rhs in pairs:
    print(lhs, rhs, lhs * rhs)
