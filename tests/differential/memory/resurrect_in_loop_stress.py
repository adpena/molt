# Council #59 regression matrix, case 9/10: resurrect_in_loop_stress.
#
# High-churn stress: a tight loop that constructs, resurrects, and immediately
# re-drains the resurrecting class many times, keeping the IC type-call fast path
# permanently hot and cycling objects through the full
# Alive->ZeroTransition->Finalizing->Resurrected->final-drop state machine
# repeatedly. This is the deterministic analogue of the original at-scale
# resurrection workload that exposed the SIGSEGV. It must stay O(1) in live
# objects (each round fully drains) and crash-free; under MOLT_ASSERT_NO_LEAK the
# `live` count must NOT grow with the iteration count.
#
# STATUS: the IC SIGSEGV is FIXED (this high-churn loop runs crash-free). The
# remaining byte-divergence is the SEPARATE loop-body finalizer-drop gap (task
# #58 / design-27, parallel-session-owned): the per-round construct+del inside the
# `while` loop never lowers a per-iteration finalizer-aware DecRef, so `__del__`
# never fires and the round-drain finds nothing in `box`. NOT the IC bug.
# Marked xfail against #58 until loop-body drop placement lands; auto-flips to
# xpass-failure when fixed.
# NOTE: byte-identical on the LLVM backend (which lowers loop-body finalizer
# drops correctly) — the gap is NATIVE-Cranelift-specific drop lowering.
# MOLT_META: xfail=molt xfail_reason=#58-loop-body-finalizer-drop-gap-NATIVE-cranelift-only-not-the-IC-fix
import gc

box = []


class R:
    def __del__(self):
        box.append(self)


def run():
    rounds = 0
    total_resurrected = 0
    i = 0
    while i < 2000:
        x = R()
        del x  # resurrect into box
        # Immediately drain this round so live objects stay O(1).
        if box:
            obj = box.pop()  # take the strong ref back
            del obj          # final drop -> truly freed (run-once, inert __del__)
            total_resurrected = total_resurrected + 1
        rounds = rounds + 1
        i = i + 1
    gc.collect()
    print("rounds", rounds)
    print("total_resurrected", total_resurrected)
    print("box_empty", len(box) == 0)


run()
print("done")
