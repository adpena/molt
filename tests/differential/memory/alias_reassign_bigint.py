# RC drop-insertion over-release regression — the heap-BigInt variant of the
# alias-then-reassign accumulator (review-required variant).
#
# Here BOTH the accumulator `x` and the counter `i` are heap BigInts (`1 << 70`
# is past the inline-int window), and `y = x` aliases the accumulator before it
# is reassigned. The original review noted bench_sum carried this pattern
# latently and "survives only via inline-int decref no-op — a heap BigInt
# accumulator would crash". This test makes the carrier a real heap BigInt so a
# wrong drop of the aliased accumulator (or its alias `y`) is a genuine
# double-free, not a no-op.
#
# Correct refcounting: each iteration the OLD accumulator BigInt is released
# exactly once at the group's last use; `y` (a transparent alias) shares that one
# reference and is NOT dropped independently. Byte-identical to CPython and bounded
# RSS (no leak, no UAF) on NATIVE.
#
# NOTE (out-of-scope blocker): on LLVM this loop hits the SEPARATE, PRE-EXISTING
# loop-accumulator unboxing bug (the heap BigInt carrier is unboxed to a raw i64
# across the back-edge and truncated — the documented `apply(1<<60,7)` class,
# unrelated to drop insertion: it reproduces with `MOLT_DROPINS_OFF=1`). The
# drop-pass over-release fix this test guards is verified byte-identical on
# native; the LLVM result is gated on that independent typed-IR fix. The string
# carrier in `alias_reassign_slice.py` exercises the same alias-reassign drop path
# byte-identically on LLVM today.
def accumulate(n):
    x = 1 << 70
    i = 1 << 70
    limit = (1 << 70) + n
    while i < limit:
        y = x          # alias of the heap-BigInt accumulator
        x = y + i      # reassign; old x (a heap BigInt) dead
        i = i + 1
    return x % 1000


print(accumulate(50))
