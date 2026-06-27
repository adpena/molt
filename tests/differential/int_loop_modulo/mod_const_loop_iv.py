# P0 silent-wrong-answer regression: a function-level loop-induction-variable
# `i % <const>` whose value-range proof (`[0, const)`) marks the result a
# raw-i64 carrier, but whose native `mod` lane fell to the boxed runtime helper
# (the constant divisor is not a raw-i64 Variable) and stored the NaN-boxed
# result through the *raw* `def_var_named` store. Every consumer then read the
# box bits as a raw i64, so `print(i % 7)` emitted 9221401712017801216, ...
# instead of 0, 1, 2.
#
# The fix routes the division-family boxed result through
# `def_var_from_numeric_result` (the carrier-aware store add/sub/mul already
# use), so storage and consumption agree on the carrier.
#
# Does NOT reproduce at module scope or for add/mul/floordiv — it is specific to
# the const-divisor boxed-fallback store under a raw-carrier output name.
def f(n):
    i = 0
    while i < n:
        print(i % 7)
        i = i + 1


f(10)
