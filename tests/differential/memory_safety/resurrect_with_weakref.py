# Spine-4 Outcome 1 (Memory-Safety Ownership Lattice) — P0 resurrection/weakref
# corruption differential. Trust-root gate for docs/design/foundation/
# 55_memory_safety_ownership_lattice.md (M2 rung-3 boundary; M3 Free demotion:
# a `Free` that skipped `weakref_clear_for_ptr`, object/mod.rs:2187, on a
# resurrected object = UAF on later weakref deref).
#
# CANONICAL SIBLING: tests/differential/memory/resurrect_with_weakref.py covers
# the bare resurrection-into-a-container case (currently xfail on a SEPARATE
# weakref-subsystem live-resolve defect). THIS file adds the strictly-additive
# fact that file does NOT exercise: weakref CALLBACK ordering across resurrection.
# It is NOT a copy — it asserts the callback-vs-__del__ interleave, which is the
# precise ordering a wrong-place/wrong-order Free would corrupt.
#
# ============================ CPython is the ORACLE ==========================
# THE LOAD-BEARING CPYTHON FACT (PEP 442 `tp_finalize`, empirically verified on
# .venv/Scripts/python.exe == CPython 3.12.13):
#
#   When `__del__` RESURRECTS the object (stores self in a live global), CPython
#   runs `__del__` FIRST; because the object is now live again, it does NOT clear
#   the weakref and does NOT invoke the weakref callback. The weakref stays LIVE
#   and resolves to the same resurrected object. The callback fires — exactly
#   once — only on the LATER, real (non-resurrecting) death, at which point the
#   weakref finally reads None.
#
# (This is the inverse of the naive "weakref cleared + callback before __del__"
# intuition, which holds only for objects that truly die. Under resurrection,
# finalization precedes — and cancels — weakref teardown. CPython is the oracle.)
#
# A `Free` (direct dealloc) emitted for this object would skip the rc 0->1
# resurrection abort (object/mod.rs:2175) and the weakref clear (object/mod.rs:2187),
# producing a freed object still pointed at by a live weakref -> use-after-free.
# The ownership lattice makes that `Free` UNREPRESENTABLE: HasWeakrefs + MayResurrect
# force a finalizer-aware DecRef (doc 55 §2.3).
#
# EXACT EXPECTED OUTPUT (CPython 3.12.13; molt must match byte-for-byte post-build):
#   alive_before True
#   trace_after_resurrect ['del']
#   box_len 1
#   weakref_live_after_resurrect True
#   weakref_same_object True
#   trace_after_final ['del', 'cb']
#   weakref_dead_after_final True
#   done
# =============================================================================
import weakref
import gc

box = []
trace = []


class R:
    # No __init__ -> inherited object.__init__ marker fn_ptr is the cached ctor
    # (the warm type-call IC path that historically triggered the resurrection
    # SIGSEGV). __del__ resurrects by stashing self in the module-global `box`.
    def __del__(self):
        trace.append("del")
        box.append(self)


def cb(ref):
    # Weakref callback. Under CPython resurrection semantics this fires ONLY on
    # the final (non-resurrecting) death, never at the first __del__.
    trace.append("cb")


def run():
    x = R()
    w = weakref.ref(x, cb)
    print("alive_before", w() is x)        # live object -> weakref resolves to it

    del x                                   # sole local ref gone -> rc 0
    gc.collect()
    # __del__ ran and resurrected; callback did NOT fire; weakref still resolves.
    print("trace_after_resurrect", trace)
    print("box_len", len(box))
    print("weakref_live_after_resurrect", w() is not None)
    print("weakref_same_object", w() is box[0])

    # Final death: drop the resurrecting reference. NOW the callback fires once
    # and the weakref clears to None.
    box.clear()
    gc.collect()
    print("trace_after_final", trace)
    print("weakref_dead_after_final", w() is None)


run()
print("done")
