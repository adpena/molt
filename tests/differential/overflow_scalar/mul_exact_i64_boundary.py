# CheckedMul must use the real signed-i64 overflow boundary, not the 47-bit
# inline-int window. Values below 2^63 remain exact; values at/above 2^63 must
# promote to Python bigint instead of wrapping.

pairs = [
    ((1 << 31) - 1, 1 << 32),  # below i64::MAX
    (1 << 31, 1 << 32),        # exactly 2^63, over signed i64
    ((1 << 62) - 1, 2),        # below 2^63
    (1 << 62, 2),              # exactly 2^63
]

for lhs, rhs in pairs:
    print(lhs, rhs, lhs * rhs)
