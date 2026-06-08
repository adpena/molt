# Council #59 regression matrix, case 6/10: resurrect_with_weakref.
#
# Weakref clear ORDER across resurrection (council §D "weakref clear order").
# In `dec_ref_ptr`, `weakref_clear_for_ptr` runs AFTER
# `maybe_run_object_finalizer` and ONLY on the true-death path: when `__del__`
# resurrects (finalizer returns true -> early return), the weakrefs are NOT
# cleared (the object is alive again, so its weakrefs must keep resolving). On
# the LATER real death the weakrefs are cleared exactly once. A weakref cleared
# at the zero-transition (before the resurrection check) would make a live
# resurrected object's weakref spuriously return None; a double-clear on final
# death would be a UAF. This verifies CPython-identical behavior: weakref
# resolves to the live object after resurrection, to None after final death.
#
# STATUS: the IC SIGSEGV is FIXED and the object DOES resurrect into `box`
# correctly. The remaining byte-divergence is a SEPARATE, pre-existing and BROAD
# weakref-subsystem defect (NOT the IC bug, NOT resurrection-specific):
#   (1) `weakref.ref(a); a.()` does not resolve to a live referent in compiled
#       code (a minimal `a=C(); w=weakref.ref(a); w() is a` returns False), and
#   (2) molt's `gc.collect()` (`weakref_collect_for_gc`) clears the weakref of any
#       target not DIRECTLY bound as a module-global value, even when the target is
#       reachable through a container (`box`) — so a live resurrected object's
#       weakref is spuriously cleared. CPython only clears weakrefs to objects it
#       actually collects (unreachable cycles).
# This case cannot meaningfully validate the resurrection weakref-clear ORDER on
# molt until the weakref subsystem resolves live referents and traces container
# reachability. Marked xfail against the weakref-subsystem baton; auto-flips to
# xpass-failure when weakref resolution is fixed.
# MOLT_META: xfail=molt xfail_reason=weakref-subsystem-live-resolve+gc-reachability-not-the-IC-fix
import weakref
import gc

box = []


class R:
    # No __init__ -> inherited object.__init__ marker fn_ptr is the cached ctor.
    def __del__(self):
        box.append(self)


def run():
    x = R()
    w = weakref.ref(x)
    del x  # resurrect into box; the weakref must STILL resolve (object alive)
    gc.collect()
    print("after_resurrect alive", w() is not None)
    print("box_len", len(box))
    # The weakref resolves to the SAME resurrected object.
    print("same_object", w() is box[0])
    # Final death: weakref clears exactly once -> resolves to None.
    box.clear()
    gc.collect()
    print("after_final dead", w() is None)


run()
print("done")
