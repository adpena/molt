# Weakref-during-resurrection (task #59, council-enumerated mechanism).
#
# In `dec_ref_ptr`, `weakref_clear_for_ptr` runs AFTER `maybe_run_object_finalizer`
# only on the TRUE-death path: when `__del__` resurrects (finalizer returns true ->
# early return), weakrefs are NOT cleared (the object is alive again, so its
# weakrefs must keep resolving). On the LATER real death the weakrefs are cleared
# exactly once. This test drives a no-`__init__`, `__del__`-resurrecting class
# (the IC-marker crash trigger from task #59) with a live weakref across the
# resurrection boundary and verifies CPython-identical weakref behavior: the
# weakref resolves to the resurrected object after the first death, and to None
# after the final death — no double-clear, no use-after-clear, no crash.
import weakref

box = []


class R:
    # No __init__ -> inherited object.__init__ marker fn_ptr is the cached ctor init.
    def __del__(self):
        box.append(self)


def run():
    # Build several so the IC type-call fast path is populated and reused (the 2nd+
    # construction is where the marker was previously mis-called).
    refs = []
    i = 0
    while i < 25:
        x = R()
        w = weakref.ref(x)
        refs.append(w)
        del x  # resurrect into box; weakref must STILL resolve (object is alive)
        i = i + 1

    # After resurrection, every weakref resolves to the live resurrected instance.
    alive_via_weakref = 0
    for w in refs:
        if w() is not None:
            alive_via_weakref = alive_via_weakref + 1
    print("alive_via_weakref", alive_via_weakref)
    print("box_len", len(box))

    # Now truly destroy each resurrected instance (final drop). __del__ already ran
    # once, so the second drop frees it and clears its weakref exactly once.
    while box:
        obj = box.pop()
        del obj

    dead_via_weakref = 0
    for w in refs:
        if w() is None:
            dead_via_weakref = dead_via_weakref + 1
    print("dead_via_weakref", dead_via_weakref)


run()
print("done")
