# Spine-4 Outcome 1 (Memory-Safety Ownership Lattice) — P0 resurrection /
# finalizer-ordering / exception-unwind corruption differential. Trust-root gate
# for docs/design/foundation/55_memory_safety_ownership_lattice.md (the VERIFIED
# OPEN SUB-CASE at doc:239-273, council "Finding 9": the exception-unwind
# release-placement leak — rung-3 placement must emit the finalizer-aware DecRef
# on the exception-transfer-to-EXIT arc, not only the normal-flow / phi-handler
# edges; drop_insertion.rs:1130-1135 + exception_arcs_for_block :1099-1106).
#
# CANONICAL SIBLING: tests/differential/memory/resurrect_during_exception_unwind.py
# covers the plain unwind-resurrection (frame-local dies on `raise`, IC-warm loop).
# THIS file is strictly-additive: it adds an ORDERED INNER REF (a Child the
# resurrecting object owns via a field) plus a `finally` clause, so it exercises
# BOTH boundary classes that overlap on the exception edge —
# MayResurrect AND InnerRefOrdering (doc 55 §1.3) — and pins the exact unwind
# ordering (finally, then handler, then __del__) that a wrong-place release
# would corrupt. NOT a copy.
#
# ============================ CPython is the ORACLE ==========================
# THE LOAD-BEARING CPYTHON FACTS (empirically verified on .venv/Scripts/python.exe
# == CPython 3.12.13):
#
#   1. The raising frame's local `obj` is held alive by the in-flight traceback
#      while the exception propagates. CPython therefore runs `obj.__del__` AFTER
#      the `finally` block AND AFTER the `except` handler body have executed — at
#      the point the handler frame finally releases the traceback — NOT
#      synchronously at the `raise` site. Trace order:
#        R_init, pre_raise, finally_ran, handler:unwind-boom, R_del
#   2. On that first (resurrecting) __del__, the object's ordered inner ref
#      (`self.child`) is NOT released: the object is live again, so its fields
#      stay valid and reachable through the resurrected object.
#   3. The inner `Child` is released — and its own __del__ runs — only on the
#      FINAL death (box.clear(); gc), AFTER the resurrected parent. Cascade order:
#        ..., R_del, child_del:k
#
# A release placed at the normal-flow last-use (drop_insertion.rs:1130-1135) is
# never reached when the frame leaves via the exception edge -> the finalizer-
# aware DecRef is dropped on the exception-transfer-to-exit arc -> a LEAK
# (fail-closed, NOT corruption, per doc 55:250-258). A `Free` here would
# additionally skip the resurrection abort and the ordered child release -> UAF
# on the child. The lattice forces a boundary-placed finalizer-aware DecRef on
# EVERY exit edge including the exception arc.
#
# EXACT EXPECTED OUTPUT (CPython 3.12.13; molt must match byte-for-byte post-build):
#   trace ['R_init', 'pre_raise', 'finally_ran', 'handler:unwind-boom', 'R_del']
#   caught True
#   box_len 1
#   child_alive_via_resurrected k
#   trace_final ['R_init', 'pre_raise', 'finally_ran', 'handler:unwind-boom', 'R_del', 'child_del:k']
#   after_final box_len 0
#   done
# =============================================================================
import gc

box = []
trace = []


class Child:
    def __init__(self, tag):
        self.tag = tag

    def __del__(self):
        # Released only on the parent's FINAL death, after the resurrected parent.
        trace.append("child_del:" + self.tag)


class R:
    # No __init__-free: R owns an ordered inner ref (`child`). HEADER_FLAG_HAS_PTRS
    # is set for this layout, so InnerRefOrdering = MayFinalize ∧ has-ptr-fields
    # is TRUE (doc 55 §1.3) on top of MayResurrect.
    def __init__(self):
        self.child = Child("k")
        trace.append("R_init")

    def __del__(self):
        trace.append("R_del")
        box.append(self)               # resurrect during unwind; child stays live


def make_and_raise():
    obj = R()                          # frame local; dies when this frame unwinds
    trace.append("pre_raise")
    raise RuntimeError("unwind-boom")  # obj's release lands on the exception arc


def run():
    caught = False
    try:
        try:
            make_and_raise()
        finally:
            # Runs DURING unwind, BEFORE the resurrecting __del__ (the traceback
            # still holds `obj` alive at this point).
            trace.append("finally_ran")
    except RuntimeError as exc:
        caught = True
        trace.append("handler:" + str(exc))
    gc.collect()
    print("trace", trace)
    print("caught", caught)
    print("box_len", len(box))
    # The resurrected object's ordered inner ref survived the unwind intact.
    print("child_alive_via_resurrected", box[0].child.tag)

    # Final death: parent released, THEN child (cascade order), leak-clean.
    box.clear()
    gc.collect()
    print("trace_final", trace)
    print("after_final box_len", len(box))


# Warm the construction-site type-call IC inside make_and_raise (the marker-call
# fast path) by unwinding the resurrection many times before the measured run —
# this is the at-scale shape that historically exposed the IC SIGSEGV.
i = 0
while i < 50:
    try:
        make_and_raise()
    except RuntimeError:
        pass
    i = i + 1
box.clear()
trace.clear()
gc.collect()

run()
print("done")
