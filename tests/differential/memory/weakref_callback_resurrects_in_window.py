# Council #1 P0 (memory): weakref callbacks run while their target is LIVE.
#
# When a weakref's referent is collected, CPython runs `tp_finalize` (`__del__`)
# and then `PyObject_ClearWeakRefs` (the weakref callbacks) with the object's
# storage LIVE — the resurrection window spans the whole finalize + weakref-clear
# span. molt previously dropped the finalizer's temporary revival reference
# BEFORE clearing weakrefs, so a weakref callback ran with its target at refcount
# 0; any work the callback did that re-touched the dying object's storage (or
# that resurrected a still-live sibling and kept using it) raced a free → a
# use-after-free / SIGSEGV that was latent because no test exercised a callback
# doing real work during that exact window.
#
# This drives a weakref callback that, during the finalization window:
#   1. confirms the weakref itself is already dead (CPython contract: `w()` is
#      None inside its own callback),
#   2. allocates fresh objects (the finalization window must support allocation),
#   3. RESURRECTS a still-live sibling object by stashing it into a global
#      container and immediately keeps using it (a UAF on the sibling's storage
#      if the window mismanaged refcounts), and
#   4. forces a re-entrant `gc.collect()` from inside the callback.
# Then it asserts byte-identical CPython output and that the resurrected sibling
# is fully intact and usable. A correct revival window keeps every object's
# storage valid; the old rc=0 window corrupted it.
#
# Run BOTH:
#   * `molt diff` (this file)      -> byte-identical to CPython 3.12/3.13/3.14.
#   * MOLT_ASSERT_NO_LEAK=1 under  -> bounded RSS, all objects ultimately freed;
#     the extra revival window must not leak on the non-resurrect path.
import weakref
import gc

log = []
survivors = []


class Payload:
    def __init__(self, tag):
        self.tag = tag
        self.data = [tag, tag]


def make_callback(spawn_tag, sibling_holder):
    def on_dead(w):
        # 1. The weakref is dead during its own callback (CPython contract).
        log.append(("weakref_dead", w() is None))
        # 2. Allocate during the finalization window.
        fresh = Payload(spawn_tag)
        log.append(("spawned", fresh.tag, len(fresh.data)))
        # 3. Resurrect a still-live sibling into a global and keep using it.
        sib = sibling_holder[0]
        if sib is not None:
            survivors.append(sib)
            sib.data.append("touched-in-callback")
            log.append(("survived", sib.tag, list(sib.data)))
        # 4. Re-entrant collection from inside the callback.
        gc.collect()

    return on_dead


def run():
    a = Payload("A")
    b = Payload("B")
    holder = [b]  # the callback for A's death grabs and resurrects b
    wa = weakref.ref(a, make_callback("spawn-from-A", holder))
    del a  # A dies -> wa callback fires inside the finalization window
    gc.collect()
    # After A's death the weakref resolves to None (its callback already fired).
    log.append(("after_del_a", wa() is None))

    print("survivors", len(survivors))
    print("survivor_tag", survivors[0].tag if survivors else None)
    # The resurrected sibling's storage is intact: mutate and read it again.
    if survivors:
        survivors[0].data.append("Z")
        print("survivor_data", survivors[0].data)

    # Drop every remaining reference to the resurrected sibling: it must die
    # cleanly now (no double-free, leak-clean).
    holder.clear()
    survivors.clear()
    del b
    gc.collect()
    for entry in log:
        print("log", entry)


run()
print("done")
