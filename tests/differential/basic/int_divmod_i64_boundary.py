"""Purpose: differential coverage for signed integer // and % at the i64 boundary.

Regression for the raw-i64 division UB class (finding #17). Cranelift `sdiv`/`srem`
TRAP on `i64::MIN / -1`, and the mathematically-correct `i64::MIN // -1 == 2**63`
overflows i64 (must become a bigint), while `i64::MIN % -1 == 0`. This drives those
corners plus a floor/mod sign matrix through DYNAMIC (non-const-folded) operands so
the native integer divide/mod lanes are actually exercised, not SCCP-folded, and
compares the results against CPython.
"""


def fdiv(a, b):
    return a // b


def fmod(a, b):
    return a % b


def dyn(x):
    # Defeat constant folding: route the value through a runtime-built list so
    # the operands reach the runtime divide/mod lanes instead of being folded.
    return [x, -x][0]


if __name__ == "__main__":
    int_min = dyn(-(2**63))
    int_max = dyn(2**63 - 1)
    neg_one = dyn(-1)
    one = dyn(1)

    # The signed-division overflow corner: quotient 2**63 is not representable
    # as i64 and must be produced as a bigint; the remainder is 0.
    print("intmin_floordiv_negone", fdiv(int_min, neg_one))
    print("intmin_mod_negone", fmod(int_min, neg_one))
    print("intmin_floordiv_one", fdiv(int_min, one))
    print("intmin_mod_one", fmod(int_min, one))
    print("intmax_floordiv_negone", fdiv(int_max, neg_one))
    print("intmax_mod_negone", fmod(int_max, neg_one))

    # Floor/mod sign matrix over dynamic operands (Python floors toward -inf and
    # the remainder takes the divisor's sign).
    xs = [7, -7, 8, -8, 2**63 - 1, -(2**63)]
    ys = [3, -3, 2, -2, -1, 1]
    for a in xs:
        for b in ys:
            print("fd", a, b, fdiv(dyn(a), dyn(b)))
            print("fm", a, b, fmod(dyn(a), dyn(b)))
