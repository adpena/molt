# P0 memory-safety differential for the INT-lane unification (BCE non-elision
# invariant). A full-range int accumulator (RawI64FullDeopt: a CheckedAdd /
# CheckedMul result, full i64 range, NOT 47-bit-window-proven) is used as a
# list index. The bounds check MUST NOT be elided.
#
# This is Memory-Safety Gate 2 of docs/design/int_lane_unification.md, the BCE
# DECOUPLING invariant. The hazard: if Bounds-Check Elimination ever trusted the
# raw-i64 carrier proof (the fact that promotes the accumulator to a raw machine
# i64) as an index-safety proof, a full-range accumulator used as an index would
# skip the bounds check on the fast lane and, when it walks past the container
# length, perform a SILENT OUT-OF-BOUNDS HEAP READ (P0 memory corruption).
#
# BCE must instead consult the strictly-narrower `proves_index_in_bounds_
# conservatively` query, which proves `0 <= i < len` AND that the index range
# fits a conservative window — a full-range RawI64FullDeopt carrier can never
# satisfy it. The correct behavior is therefore a deterministic IndexError (when
# the index walks out of range) or the correct element (when an explicit guard
# keeps it in range), matching CPython exactly.
#
# Every case below uses an accumulator whose value the value-range analysis
# CANNOT bound (it is genuinely unbounded / full-range), so a correct BCE leaves
# the runtime check in. Observing the results (and the IndexError boundary)
# forces any wrongful elision to diverge from CPython.
#
# Expected CPython 3.12 output (verified on .venv/Scripts/python.exe, Py3.12.13):
#   walk [10, 11, 13, 'IDXERR@6']
#   guarded [100, 200, 400, -1, -1, -1, -1, -1, -1, -1]
#   mul [8, 9, 'OOB@6', 'OOB@24', 'OOB@120', 'OOB@720', 'OOB@5040']


# --- Case A: unguarded full-range accumulator index walks OOB -----------------
# `acc` is the cumulative sum 0,0,1,3,6,10,... (a CheckedAdd accumulator). It is
# used DIRECTLY as `lst[acc]` with no guard. acc=6 exceeds len(lst)=5, so CPython
# raises IndexError. A wrongful BCE elision would instead read OOB heap memory.
# (Note acc skips index 2 — lst[2]==12 never appears — confirming acc really is
# the unbounded running sum, not a 0..n loop counter a range analysis could
# bound.)
def indexed_walk():
    lst = [10, 11, 12, 13, 14]  # len 5
    acc = 0
    out = []
    i = 0
    while True:
        acc = acc + i  # CheckedAdd; full-range carrier, NOT 47-bit-proven
        try:
            out.append(lst[acc])  # index must keep its runtime bounds check
        except IndexError:
            out.append("IDXERR@" + str(acc))
            break
        i = i + 1
    return out


print("walk", indexed_walk())


# --- Case B: guarded full-range accumulator index stays correct ---------------
# Same accumulator, but an explicit `acc < len(lst)` guard keeps the access
# in-bounds. Proves the decoupled conservative query does NOT break a
# genuinely-safe (but full-range-carried) index: the access still returns the
# correct element; the only cost is the retained runtime check (a documented,
# acceptable perf trade-off, NOT a correctness change).
def guarded_walk():
    lst = [100, 200, 300, 400, 500]
    acc = 0
    out = []
    for i in range(10):
        acc = acc + i  # CheckedAdd accumulator: 0,0,1,3,6,10,...
        if acc < len(lst):
            out.append(lst[acc])
        else:
            out.append(-1)
    return out


print("guarded", guarded_walk())


# --- Case C: CheckedMul accumulator index ------------------------------------
# A product accumulator (CheckedMul) outpaces the container length immediately:
# 1,1,2,6,24,120,... The multiply lane has the identical non-elision obligation
# as the add lane. Guarded so the result is deterministic; the point is that the
# raw-i64 product carrier must never be treated as an in-bounds-index proof.
def mul_index():
    lst = [7, 8, 9]  # len 3
    acc = 1
    out = []
    for step in range(1, 8):
        acc = acc * step  # CheckedMul; full-range carrier
        if acc < len(lst):
            out.append(lst[acc])
        else:
            out.append("OOB@" + str(acc))
    return out


print("mul", mul_index())
