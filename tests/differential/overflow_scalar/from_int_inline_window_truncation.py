# P0 silent-integer-truncation differential for the `MoltObject::from_int`
# inline-window class. `from_int` only accepts the 47-bit NaN-box window
# [-2**46, 2**46); fed a larger value it masks mod 2**47 (release) — a silent
# wrong answer. Numeric builtins that boxed a `to_i64(...)` result through
# `from_int` truncated any exact int in [2**46, 2**63): fit-i64 BigInts AND
# exact-integer floats up to 2**53. The fix routes them through the full-range
# `int_bits_from_i64` / BigInt passthrough instead.
#
# Every value below is > 2**46 (= 70368744177664) so a regression to `from_int`
# masks it mod 2**47 and diverges from CPython. `big` (~2**60) is the task's
# canonical fit-i64 BigInt.
import math
import operator

BIG = 1152921504606846977          # ~2**60, a heap BigInt that fits in i64
W = 2**46                          # first int OUTSIDE the inline window
FBIG = 1e14                        # exact-integer float, 2**46 < 1e14 < 2**53

# --- math.trunc / floor / ceil on a fit-i64 BigInt --------------------------
print("trunc_big", math.trunc(BIG))
print("floor_big", math.floor(BIG))
print("ceil_big", math.ceil(BIG))

# --- math.trunc / floor / ceil on an exact-integer float past 2**46 ---------
print("trunc_f", math.trunc(FBIG), type(math.trunc(FBIG)).__name__)
print("floor_f", math.floor(FBIG), type(math.floor(FBIG)).__name__)
print("ceil_f", math.ceil(FBIG), type(math.ceil(FBIG)).__name__)

# --- floats past i64 (1e19 > 2**63) must box to BigInt, not overflow --------
print("trunc_1e19", math.trunc(1e19))
print("floor_1e19", math.floor(1e19))
print("ceil_1e19", math.ceil(1e19))

# --- math.trunc / floor / ceil at the exact window boundary 2**46 -----------
print("trunc_W", math.trunc(W), math.trunc(float(W)))
print("floor_W", math.floor(W), math.floor(float(W)))

# --- round() int branch (no ndigits / None / >=0 / <0) ----------------------
print("round_big", round(BIG))
print("round_big_none", round(BIG, None))
print("round_big_pos", round(BIG, 3))
print("round_big_neg", round(BIG, -3))
print("round_W", round(W), round(W, 5))

# --- unary plus / operator.pos / operator.index on a fit-i64 BigInt ---------
print("pos_big", +BIG)
print("operator_pos_big", operator.pos(BIG))
print("operator_index_big", operator.index(BIG))
print("pos_W", +W, operator.pos(W))

# --- range.index / range.count with large element values & indices ----------
print("range_index_huge", range(0, 2**62).index(2**61))
print("range_count_huge", range(0, 2**62).count(2**61))
print("range_index_bigvals", range(BIG, BIG + 10).index(BIG + 7))
print("range_count_bigvals", range(BIG, BIG + 10).count(BIG + 3))
print("range_index_W", range(0, 2**50).index(W))
