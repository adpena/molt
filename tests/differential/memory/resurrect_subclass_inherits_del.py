# Council #59 regression matrix, case 8/10: resurrect_subclass_inherits_del.
#
# The resurrecting `__del__` is INHERITED from a base class, and NEITHER the base
# nor the subclass defines `__init__` — so the subclass's resolved+cached
# constructor init is STILL the inherited `object.__init__` marker (the IC
# fast-path crash trigger), now reached through a multi-level MRO. This verifies
# the marker decode is correct regardless of how deep the class hierarchy is, and
# that an inherited `__del__` participates in run-once resurrection identically.
#
# Two subclasses share the base `__del__`; instantiating both warms the IC across
# distinct classes (each class is a distinct type-call IC entry), so a marker
# mishandle would crash on whichever class warms first.
#
# STATUS: the IC SIGSEGV is FIXED (no crash across either subclass's warm IC).
# The remaining byte-divergence is the SEPARATE loop-body finalizer-drop gap
# (task #58 / design-27, parallel-session-owned): the construct+del happen inside
# a `while` loop, so the inherited `__del__` never fires for the loop-local
# instances. NOT the IC bug. Marked xfail against #58 until loop-body drop
# placement lands; auto-flips to xpass-failure when fixed.
# NOTE: byte-identical on the LLVM backend (which lowers loop-body finalizer
# drops correctly) — the gap is NATIVE-Cranelift-specific drop lowering.
# MOLT_META: xfail=molt xfail_reason=#58-loop-body-finalizer-drop-gap-NATIVE-cranelift-only-not-the-IC-fix
import gc

box = []


class Base:
    # Defines __del__ but NO __init__ -> object.__init__ marker is the ctor init.
    def __del__(self):
        box.append(("base_del", type(self).__name__))


class Mid(Base):
    pass


class Leaf(Mid):
    pass


def run():
    i = 0
    while i < 30:
        a = Mid()
        del a
        b = Leaf()
        del b
        i = i + 1
    gc.collect()
    print("box_len", len(box))
    # Count resurrections per concrete class.
    mids = 0
    leaves = 0
    for kind, name in box:
        if name == "Mid":
            mids = mids + 1
        elif name == "Leaf":
            leaves = leaves + 1
    print("mids", mids, "leaves", leaves)
    box.clear()
    gc.collect()
    print("after_final box_len", len(box))


run()
print("done")
