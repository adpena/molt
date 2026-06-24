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
# STATUS: PASSING. The #59 IC marker SIGSEGV was fixed earlier; the remaining
# finalizer exception-state boundary was closed by aligning
# `maybe_run_object_finalizer` with CPython `PyObject_CallFinalizer`
# (fetch-and-clear the in-flight exception before `__del__`, restore it after):
# the object is appended before mainline observes `box`, and the `__del__`
# ValueError is swallowed (written unraisable) even when resurrection and
# gc.collect compose. stdout is byte-identical and stderr matches by exception
# signature; the prior `xfail=molt` is removed now that both boundaries match.
# MOLT_META: stderr=exception_signature
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
