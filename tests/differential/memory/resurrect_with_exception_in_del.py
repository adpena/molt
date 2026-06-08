# Council #59 regression matrix, case 7/10: resurrect_with_exception_in_del.
#
# An exception raised inside `__del__` must be SWALLOWED (CPython prints an
# "Exception ignored ..." traceback to stderr and continues; the exception never
# propagates into the mainline). This case combines that with resurrection: the
# first `__del__` resurrects the object AND raises; the raise must not abort the
# program, the resurrection must still take effect, and the object must remain
# usable. stdout must be byte-identical to CPython; stderr is compared by
# exception SIGNATURE (type+message) since the traceback frame/address formatting
# differs across engines.
#
# STATUS: the IC SIGSEGV is FIXED. The remaining divergence is a SEPARATE,
# pre-existing finalizer exception-state defect (NOT the IC bug): an exception
# raised in `__del__` is NOT reliably swallowed in molt — whether it is swallowed
# is COMPOSITION-DEPENDENT (the same no-`__init__` raise-in-`__del__` class
# swallows correctly in isolation but PROPAGATES and aborts the program when other
# finalizer classes precede it in the same program; observed crash on the FIRST
# raise here). CPython always swallows (prints an "Exception ignored while calling
# deallocator" warning to stderr, continues). The likely locus is the
# exception-baseline / pending-exception-stack interaction during finalizer
# dispatch (object/mod.rs maybe_run_object_finalizer exception-clear, reached via
# gc.collect() -> weakref_collect_for_gc). Marked xfail against the
# finalizer-exception-swallow baton; auto-flips to xpass-failure when fixed.
# stderr is compared by exception signature (frame/address formatting differs).
# MOLT_META: stderr=exception_signature xfail=molt xfail_reason=finalizer-exception-swallow-composition-dependent-not-the-IC-fix
import gc

box = []


class R:
    # No __init__ -> inherited object.__init__ marker fn_ptr is the cached ctor.
    def __del__(self):
        box.append(self)
        raise ValueError("boom in del")


def run():
    x = R()
    del x  # __del__ resurrects (append) THEN raises -> exception swallowed
    gc.collect()
    # Mainline continues uninterrupted; the resurrection took effect.
    print("after_del box_len", len(box))
    print("resurrected_usable", isinstance(box[0], R))
    # Final drop: __del__ already ran once (run-once), object truly freed.
    box.clear()
    gc.collect()
    print("after_final box_len", len(box))


run()
print("done")
