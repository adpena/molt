# MOLT_META: xfail=molt xfail_reason=finalizer-resurrection-explicit-del-boundary
# Leak-gauge soundness under finalizer resurrection (task #56 / council B).
#
# `dec_ref_ptr` bumps the DEALLOC_COUNT / DEALLOC_BYTES / per-type dealloc
# counters at the rc=1->0 transition. But `__del__` may RESURRECT the object
# (stash `self` somewhere, re-incrementing its refcount), in which case
# `dec_ref_ptr` returns WITHOUT freeing. If the dealloc were counted at the
# zero-transition (before the resurrection check), a resurrected object would be
# counted as deallocated for a free that never happened, so
# `live = ALLOC_COUNT - DEALLOC_COUNT` would UNDER-count live objects — an
# unsound leak gauge (a phantom "no leak" while the object is resurrected-alive).
#
# This test drives an object through BOTH states in one deterministic program:
#   1. `del r`     -> refcount 0 -> __del__ runs -> object resurrects into `_box`
#                     (it is ALIVE again; the gauge must NOT count it dealloc'd).
#   2. `del obj`   -> the last reference (taken back out of `_box`) drops ->
#                     __del__ runs again (the `revived` path is inert) -> the
#                     object is TRULY destroyed (counted dealloc'd exactly here).
#
# STATUS: expected-fail until the explicit `del` finalizer/resurrection boundary
# runs. Today Molt reaches `after-first-drop box_len=0` and then raises
# `IndexError: pop from empty list`, so the leak-gauge accounting contract cannot
# yet be exercised by this program. The intended fix still moves dealloc-counter
# increments to AFTER the `maybe_run_object_finalizer` resurrection check, so
# DEALLOC_COUNT means "objects actually freed".
#
# Run BOTH:
#   * `molt diff` (this file)            -> byte-identical to CPython 3.14.
#   * MOLT_ASSERT_NO_LEAK=1 under        -> live == EXPECTED_LIVE_OBJECTS at exit
#     `safe_run.py --rss-mb 64`             (clean pass; exit 0). A gauge that
#                                            under-counts the resurrected-alive
#                                            object cannot reach the correct final
#                                            count without the move.
_box = []


class Resurrector:
    def __init__(self, tag):
        self.tag = tag
        self.revived = False

    def __del__(self):
        if not self.revived:
            # First finalization: resurrect by stashing self in a live container.
            self.revived = True
            _box.append(self)
            print("del-resurrect", self.tag)
        else:
            # Second (final) finalization: the object is truly destroyed now.
            print("del-final", self.tag)


def run():
    r = Resurrector(7)
    del r  # refcount -> 0 -> __del__ -> resurrected into _box (ALIVE again)
    print("after-first-drop box_len=%d" % len(_box))
    obj = _box.pop()  # take the sole strong reference back out of the container
    print("took-from-box box_len=%d" % len(_box))
    del obj  # drop the last reference -> __del__ again -> TRULY freed
    print("after-final-drop")


run()
print("done")
