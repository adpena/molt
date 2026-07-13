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
# Explicit collection has no independent weakref sweep: RC-confirmed destruction
# and the cycle collector's proven-unreachable set are the only clear authorities.
# This therefore exercises the real resurrection ordering without an xfail lane.
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
