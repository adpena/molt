# Council #59 regression matrix, case 10/10: resurrect_during_exception_unwind.
#
# The resurrecting object is a LOCAL whose final reference is dropped while an
# exception is UNWINDING the frame. CPython releases a frame's locals as the
# stack unwinds past them; if that local is the sole reference to a
# `__del__`-resurrecting instance, the finalizer runs (and resurrects) ON THE
# EXCEPTION-UNWIND PATH, not the normal return path. This exercises the release
# lowering of the exception edge (council §D "inner-ref cascade order" +
# "LLVM DecRef lowering ... on exception edges"): the resurrection must take
# effect, the in-flight exception must still propagate correctly to the handler,
# and there must be no SIGSEGV / no double-finalize / no leak.
import gc

box = []


class R:
    # No __init__ -> object.__init__ marker is the cached ctor (IC crash trigger).
    def __del__(self):
        box.append(self)


def make_and_raise():
    # `obj` is a frame local. When `raise` unwinds this frame, `obj`'s last
    # reference is dropped during unwind -> __del__ -> resurrect into box.
    obj = R()
    raise RuntimeError("unwind-boom")


def run():
    caught = False
    try:
        make_and_raise()
    except RuntimeError as exc:
        caught = True
        print("caught", str(exc))
    gc.collect()
    # The local resurrected during unwind; the exception still reached us.
    print("caught_flag", caught)
    print("box_len", len(box))
    print("resurrected_usable", isinstance(box[0], R))
    box.clear()
    gc.collect()
    print("after_final box_len", len(box))


# Run the unwind-resurrection many times so the type-call IC is warm at the
# construction site inside make_and_raise (the marker-call fast path).
i = 0
while i < 50:
    try:
        make_and_raise()
    except RuntimeError:
        pass
    i = i + 1
box.clear()
gc.collect()

run()
print("done")
