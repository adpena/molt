# Council #59 regression matrix, case 3/10: resurrect_once_N1000.
#
# The at-scale stress: N=1000 instantiations of the no-`__init__`,
# `__del__`-resurrecting class. The IC type-call fast path is warm for ~999 of
# them, so this is the heaviest exercise of the formerly-crashing marker-call
# path. With the single-decode-authority fix it must complete crash-free,
# byte-identical to N1/N10, and leak-clean (every resurrected instance is freed
# exactly once on the final drain, so `live` returns to the immortal floor under
# MOLT_ASSERT_NO_LEAK).
#
# Lifecycle invariant (council §D): __del__ once per object; resurrected objects
# usable; final destruction on the later drop; NO_LEAK counts actual destruction.
#
# STATUS: the IC SIGSEGV is FIXED (this completes crash-free at N=1000 — verified;
# it previously SIGSEGV'd past ~10 iterations). The remaining byte-divergence is a
# SEPARATE, pre-existing defect: the loop-body finalizer-drop gap (task #58 /
# design-27 ownership lattice, parallel-session-owned). A construct-and-`del`
# inside a loop never lowers a per-iteration finalizer-aware DecRef, so the
# loop-local instances leak (profile: alloc_object=1001, dealloc_object=0) and
# `__del__` never fires (box stays empty). NOT the IC bug — STRAIGHT-LINE
# construct+del finalizes correctly regardless of IC warmth. (The leak is bounded
# at N and stays under the MOLT_ASSERT_NO_LEAK ceiling, so the gauge does not
# catch it; the byte-compare is the detector.) Marked xfail against #58 until
# loop-body drop placement lands; auto-flips to xpass-failure when fixed.
# NOTE: byte-identical on the LLVM backend (which lowers loop-body finalizer
# drops correctly) — the gap is NATIVE-Cranelift-specific drop lowering.
# MOLT_META: xfail=molt xfail_reason=#58-loop-body-finalizer-drop-gap-NATIVE-cranelift-only-not-the-IC-fix
import gc

N = 1000
box = []


class R:
    def __del__(self):
        box.append(self)


def run():
    i = 0
    while i < N:
        x = R()
        del x  # warm IC fast path; previously SIGSEGV past ~10 iterations
        i = i + 1
    gc.collect()
    print("resurrected", len(box))
    # Drain: each instance already ran __del__ once, so the final drop frees it
    # (it does NOT re-run __del__). End leak-clean.
    freed = 0
    while box:
        box.pop()
        freed = freed + 1
    gc.collect()
    print("freed", freed)
    print("box_empty", len(box) == 0)


run()
print("done")
