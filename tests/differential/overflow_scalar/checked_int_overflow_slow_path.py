# P0 memory-safety / value-correctness differential for the INT-lane unification
# (RawI64FullDeopt tier). Exercises the CheckedAdd / CheckedMul slow-path deopt:
# an int accumulator overflows i64 (sum/product past 2^63), the hardware overflow
# flag gates dispatch to the boxed slow path, and the slow path re-executes the
# failed iteration on a BigInt carrier. The result MUST be the exact
# arbitrary-precision value CPython produces — proving the slow-path re-execution
# is value-correct (a wrapped i64 reinterpreted as a Python int = silent wrong
# answer, the RawI64FullDeopt soundness contract).
#
# This is Memory-Safety Gate 1 of docs/design/int_lane_unification.md.
# RawI64FullDeopt's safety relies ENTIRELY on the overflow_peel CFG being live:
# if the peel is missing or its dispatch is unreachable, a wrapped value is
# misinterpreted as a valid Python int. Observing (printing) every accumulator
# forces any wrap to diverge from CPython.
#
# Expected CPython 3.12 output (verified on .venv/Scripts/python.exe, Py3.12.13):
#   add_total 9223372036854775848
#   add_is_int int
#   mul_fact_25 15511210043330985984000000
#   mixed_70 2361183241434822606777
#   both_fast 780 both_slow 5070602400912917605986812821504


# --- CheckedAdd slow path: sum crossing 2^63 ---------------------------------
# `total` starts at 2^63 - 5 and a `+ i` loop pushes it past i64::MAX within a
# few iterations. Every iteration after the boundary is the boxed slow path.
def sum_cross(n, start):
    total = start
    i = 0
    while i < n:
        total = total + i  # CheckedAdd; overflows i64, deopts to BigInt
        i = i + 1
    return total


START = (1 << 63) - 5
add_total = sum_cross(10, START)
print("add_total", add_total)
# The deopted result must remain a Python int (not a wrapped/native sentinel).
print("add_is_int", type(add_total).__name__)


# --- CheckedMul slow path: product crossing 2^63 -----------------------------
# A factorial product blows past i64::MAX almost immediately; 25! is far into
# BigInt territory, so the CheckedMul flag must deopt and the boxed product be
# exact.
def prod_cross(stop):
    p = 1
    for k in range(1, stop):
        p = p * k  # CheckedMul; overflows i64, deopts to BigInt
    return p


print("mul_fact_25", prod_cross(26))


# --- Mixed CheckedMul + CheckedAdd on a now-BigInt carrier --------------------
# Doubling (CheckedMul-shaped) crosses 2^63 around k=63, after which the `+ k`
# (CheckedAdd) operand is already a BigInt. Proves the slow path re-executes the
# WHOLE iteration body correctly, not just the overflowing op.
def mixed(stop):
    acc = 1
    for k in range(stop):
        acc = acc * 2  # CheckedMul
        acc = acc + k  # CheckedAdd, on a possibly-BigInt acc
    return acc


print("mixed_70", mixed(70))


# --- Interleaved fast + slow accumulators ------------------------------------
# `fast` stays small (it never leaves the raw i64 fast lane); `slow` doubles past
# 2^63 within a handful of iterations (slow lane). Proves the slow-path
# re-execution of `slow` does NOT corrupt the concurrently-live fast-lane `fast`
# (a shared-saved-value re-execution bug would clobber it).
def both():
    fast = 0
    slow = 1 << 62
    for i in range(40):
        fast = fast + i  # fast lane, stays in i64
        slow = slow + slow  # slow lane, overflows i64 fast
    return fast, slow


f, s = both()
print("both_fast", f, "both_slow", s)
