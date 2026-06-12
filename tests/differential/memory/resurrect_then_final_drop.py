# Council #59 regression matrix, case 4/10: resurrect_then_final_drop.
#
# Explicitly drives the two-phase lifecycle state machine (council §D):
#   Alive --lastDecRef--> ZeroTransition --maybe_run_finalizer-->
#   Finalizing(RAN set) --[resurrect]--> Resurrected(rc>0, RAN set, valid header)
#   --later final drop--> DestroyingWithoutSecondFinalizer --> ActualDealloc.
#
# `__del__` resurrects the FIRST time it runs. CPython's run-once finalizer
# machinery (the tp_finalize-ran flag) guarantees `__del__` is called EXACTLY
# ONCE per object lifetime: the later final drop frees the object WITHOUT
# re-invoking `__del__`. So `del_calls` stays `[False]` across both phases — the
# observable proof of run-once. The object is usable in the resurrected window;
# the final drop frees it; leak-clean. (This is the canonical run-once invariant:
# molt must NOT re-run `__del__` on the resurrected object's final destruction.)
import gc

box = []
del_calls = []


class R:
    def __init__(self):
        self.revived = False

    def __del__(self):
        del_calls.append(self.revived)
        if not self.revived:
            self.revived = True
            box.append(self)


def run():
    r = R()
    del r  # ZeroTransition -> __del__ (revived False) -> resurrect into box
    gc.collect()
    print("phase1 box_len", len(box), "del_calls", del_calls)
    # Resurrected window: object is alive and usable.
    obj = box.pop()
    print("resurrected_usable", obj.revived)
    del obj  # final drop -> __del__ (revived True) -> inert -> truly freed
    gc.collect()
    print("phase2 box_len", len(box), "del_calls", del_calls)


run()
print("done")
