"""Purpose: end-to-end soundness coverage for value-range phi-range narrowing
(task #43) — BOTH directions of the licensing structure.

A loop-header phi whose loop-carried back-edge value is re-bounded to a
phi-independent range (e.g. `x & MASK` -> `[0, MASK]`, computed under the
forward sweep's all-phis-FULL assumption) may be narrowed to the JOIN of its
incomings. This restores the raw-i64 lane for masked-shift accumulators.

The DANGER is narrowing a phi whose back-edge range actually DEPENDS on the
phi: an unbounded accumulator (`acc = acc * 2`, `total = total + i`) can exceed
the inline window and even i64, so it MUST stay boxed (bigint). If narrowing
ever fired on those, the result would silently wrap/truncate — the worst bug
class. The forward sweep computes back-edge ranges with the phi pinned at FULL,
so an accumulator's back-edge range is FULL -> JOIN is FULL -> never narrowed.

Run byte-for-byte against CPython on native + llvm. Every value below is chosen
so the masked lane (fast) and the bigint lane (must-not-narrow) produce values
that DIFFER from any wrapped/truncated result, so a mis-narrowing is caught as a
wrong answer here, not just a perf change.
"""


def show(label, value):
    print(label, repr(value))


# ── MUST narrow: masked back-edge shift accumulator (phi-independent [0, MASK]).
def masked_shift(mask, n):
    s = 1
    for _ in range(n):
        s = (s << 1) & mask
    return s


# 32-bit mask, many iterations: the value cycles within [0, mask]. The raw lane
# (post-narrowing) and the boxed lane must agree — this is the value the
# narrowing makes fast.
show("masked<<1 m=2**32-1 n=64", masked_shift((1 << 32) - 1, 64))
show("masked<<1 m=2**32-1 n=33", masked_shift((1 << 32) - 1, 33))
show("masked<<1 m=2**16-1 n=40", masked_shift((1 << 16) - 1, 40))
# Mask exactly at the inline-window edge boundary region (still well inside).
show("masked<<1 m=2**40-1 n=50", masked_shift((1 << 40) - 1, 50))


# ── MUST narrow: masked with a wider per-step shift, still bounded by the mask.
def masked_shift_by(mask, shift, n):
    s = 3
    for _ in range(n):
        s = (s << shift) & mask
    return s


show("masked<<4 m=2**32-1 n=20", masked_shift_by((1 << 32) - 1, 4, 20))
show("masked<<7 m=2**28-1 n=15", masked_shift_by((1 << 28) - 1, 7, 15))


# ── MUST narrow: AND-masked accumulator (the bit_and re-bound on a non-shift).
def masked_or_accumulate(mask, n):
    s = 0
    for i in range(n):
        s = (s + i) & mask
    return s


show("masked (s+i)&m m=255 n=100", masked_or_accumulate(255, 100))
show("masked (s+i)&m m=2**20-1 n=5000", masked_or_accumulate((1 << 20) - 1, 5000))


# ── MUST NOT narrow: unbounded doubling accumulator -> genuine bigint.
# Back-edge `acc << 1` has FULL range (operand acc is FULL under the sweep), so
# the phi must stay unproven and the value must be the exact bigint, NOT a
# 64-bit wrap. n=70 crosses the i64 boundary; n=200 is a large bigint.
def doubling(n):
    acc = 1
    for _ in range(n):
        acc = acc << 1
    return acc


show("doubling n=70", doubling(70))
show("doubling n=70 bit_length", doubling(70).bit_length())
show("doubling n=200 bit_length", doubling(200).bit_length())
show("doubling n=130 low64", doubling(130) & ((1 << 64) - 1))


# ── MUST NOT narrow: unbounded sum accumulator (the bigint_accumulator gate).
def big_sum(n):
    total = 0
    for i in range(n):
        total = total + i * i
    return total


show("big_sum n=1000", big_sum(1000))
show("big_sum n=200000", big_sum(200000))  # ~2.6e15, exceeds the 2**46 window


# ── MUST NOT narrow: multiply accumulator (factorial-like) -> huge bigint.
def fact(n):
    p = 1
    for i in range(1, n + 1):
        p = p * i
    return p


show("fact 25", fact(25))           # 25! has 26 digits — way past i64
show("fact 25 bit_length", fact(25).bit_length())


# ── Mixed: a masked phi AND an unbounded phi in the SAME loop. The masked one
# narrows (raw, fast); the unbounded one must NOT (bigint-correct). The compiler
# must treat the two header phis independently.
def mixed(mask, n):
    s = 1
    big = 1
    for _ in range(n):
        s = (s << 1) & mask
        big = big << 1
    return s, big.bit_length()


show("mixed m=2**32-1 n=80", mixed((1 << 32) - 1, 80))


if __name__ == "__main__":
    pass
