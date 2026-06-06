# RC drop-insertion over-release regression — MIXED-OWNERSHIP PHI (design 20
# §ownership; round-3 finding). A block-arg (TIR phi) is treated as carrying one
# owned `+1`: the drop pass DROPS it where it dies and TRANSFERS it where it is
# forwarded. Both halves of that transfer must be exact, or the phi's drop
# releases a reference the function does not own → `invalid object header before
# dec_ref` (SIGABRT) / SIGSEGV.
#
# Two over-release classes this exercises, each on the heap (boxed) lane:
#
#   1. BORROWED value into an OWNED phi (incoming side). `x = base; while …:
#      x = x + base` seeds the accumulator phi with the borrowed parameter `base`
#      (`+0` ABI — the caller owns it). The loop body drops the phi every
#      iteration; without a retain on the loop-ENTRY edge that decrements the
#      caller's borrow → premature free. The fix retains the borrowed value on the
#      entry edge so the phi uniformly owns a `+1`. The control `x = 0` is immune
#      (the phi is then raw/inline, never dropped).
#
#   2. OWNED value FORWARDED into a phi, then double-dropped at the join (outgoing
#      side). `x = a if c else a + a; return x + a` forwards the owned `a + a`
#      into the merge phi; the inliner's multi-block lowering made the edge-dying
#      rule drop it at the join AND at the phi's last use. The fix excludes a
#      branch-arg value from edge-dying drops (its ownership moved into the phi).
#
# ESCALATION SHAPES (round-4 Finding 1, restack-confirmed). The activation chain's
# stale base lacked main's LLVM arms for bigint floor-division (`//`) and the
# `__matmul__` dunder (`@`); on the current restacked base both are byte-identical.
# The two accumulator variants below pin that the §5 mixed-ownership-phi retain
# composes with those operators when they FEED an owned phi every iteration — the
# escalated form of class 1, where the phi-producing op is `//` (a heap-bigint
# FloorDiv temp) or `@` (a heap custom-object MatMul temp) rather than `+`.
#
# All shapes are byte-identical to CPython on LLVM (drops active) and native
# (drops gated off) and run in O(1) RSS regardless of n. Run under
# `safe_run.py --rss-mb 64`: an over-release aborts; a leak trips the RSS cap.


# ── Class 1: borrowed param seeds a loop accumulator phi ──────────────────────
def loop_add_bigint(base, n):
    x = base                 # x phi seeded with the borrowed heap-bigint param
    i = 0
    while i < n:
        x = x + base         # reads `base` every iteration; drops the old phi
        i += 1
    return x


def loop_concat_string(base, n):
    s = base                 # s phi seeded with the borrowed heap-string param
    i = 0
    while i < n:
        s = s + base
        i += 1
    return s


def loop_add_zero_control(base, n):
    x = 0                    # control: phi is inline/raw — must stay correct too
    i = 0
    while i < n:
        x = x + base
        i += 1
    return x


# ── Class 1 (escalation): heap-bigint FLOOR-DIVISION feeds the accumulator phi ──
# The accumulator phi carries a heap bigint; each iteration recomputes it through
# `//` (and `*`), both heap-temp-producing ops. The product `x * base` overflows
# the inline-int lane (it stays a real bigint for `base = 1<<60`), so `//` runs on
# the boxed lane — the exact arm the stale chain base mis-lowered. The recurrence
# `x = (x * base) // base` is the identity (`== base`), so the value is checkable
# and the loop is a pure per-iteration churn of owned bigint temps into the phi.
def loop_floordiv_bigint(base, n):
    x = base                 # x phi seeded with the borrowed heap-bigint param
    i = 0
    while i < n:
        x = (x * base) // base   # heap bigint `*` then `//`, both feed the phi
        i += 1
    return x


# ── Class 1 (escalation): a `__matmul__` (`@`) dunder feeds the accumulator phi ─
# `Box.__matmul__` mints a NEW heap `Box` each iteration; the accumulator phi
# carries that owned object and is dropped+rebound every loop. The `@` operand on
# both sides is a heap custom-class instance — the MatMul-dunder dispatch arm the
# stale chain base lacked. The recurrence keeps the wrapped value at `base.v` so
# the result is checkable.
class Box:
    def __init__(self, v):
        self.v = v

    def __matmul__(self, other):
        # idempotent on equal-valued boxes: returns a fresh Box holding self.v
        return Box(self.v if self.v == other.v else self.v + other.v)


def loop_matmul_obj(base_v, n):
    base = Box(base_v)
    x = base                 # x phi seeded with the borrowed heap-Box param
    i = 0
    while i < n:
        x = x @ base         # fresh owned Box every iteration feeds the phi
        i += 1
    return x.v


# ── Class 2: if/else value merge feeding a later consumer ─────────────────────
def merge_then_borrowed(a, c):
    # `then` arm forwards the borrowed param `a` into the merge phi; `else` arm
    # forwards a fresh owned value. The phi is later dropped by `x + a`.
    if c:
        x = a
    else:
        x = a + a
    return x + a


def merge_ternary(a, c):
    x = a if c else a + a
    return x + a


# ── Nested loops: an accumulator phi inside an outer loop ──────────────────────
def nested_loops(base, outer, inner):
    total = base
    o = 0
    while o < outer:
        inneracc = base
        k = 0
        while k < inner:
            inneracc = inneracc + base
            k += 1
        total = total + inneracc
        o += 1
    return total


BIG = 1 << 60

# Class 1 — heap bigint accumulator (the round-2 repro), n=0 and n>0.
print(loop_add_bigint(BIG, 7))
print(loop_add_bigint(BIG, 0))        # loop body never runs: returns `base` (+1 to caller)
print(loop_add_bigint(BIG, 200))

# Class 1 — heap string accumulator.
print(loop_concat_string("ab", 7))
print(loop_concat_string("ab", 0))

# Class 1 control — inline-int start (must stay correct).
print(loop_add_zero_control(BIG, 7))

# Class 1 escalation — heap-bigint floor-division accumulator (round-4 Finding 1).
print(loop_floordiv_bigint(BIG, 7))
print(loop_floordiv_bigint(BIG, 0))   # loop body never runs: returns `base`
print(loop_floordiv_bigint(BIG, 200))

# Class 1 escalation — `__matmul__` (`@`) accumulator (round-4 Finding 1).
print(loop_matmul_obj(BIG, 7))
print(loop_matmul_obj(BIG, 0))        # loop body never runs: returns base.v
print(loop_matmul_obj(BIG, 200))

# Class 2 — both arms of the if/else merge and the ternary.
print(merge_then_borrowed(BIG, True))
print(merge_then_borrowed(BIG, False))
print(merge_ternary(BIG, True))
print(merge_ternary(BIG, False))

# Nested-loop accumulator phis.
print(nested_loops(BIG, 5, 4))

# Larger iteration count to make any per-iteration leak trip the RSS cap.
print(loop_add_bigint(BIG, 5000) == BIG * 5001)
print(loop_concat_string("x", 20000) == "x" * 20001)
print(loop_floordiv_bigint(BIG, 5000) == BIG)
print(loop_matmul_obj(BIG, 5000) == BIG)
