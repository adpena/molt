# Council #59 regression matrix, case 3/10: resurrect_once_N1000.
#
# The at-scale stress: N=1000 instantiations of the no-`__init__`,
# `__del__`-resurrecting class. The IC type-call fast path is warm for ~999 of
# them, so this is the heaviest exercise of the formerly-crashing marker-call
# path. With the single-decode-authority fix it must complete crash-free,
# byte-identical to N1/N10, and leak-clean (every resurrected instance is freed
# exactly once on the final drain, so `live` returns to the immortal floor under
# MOLT_ASSERT_NO_LEAK).
#
# Lifecycle invariant (council §D): __del__ once per object; resurrected objects
# usable; final destruction on the later drop; NO_LEAK counts actual destruction.
#
# STATUS: native differential pass. The historical IC SIGSEGV and the later
# loop-body finalizer-drop gap are both fixed at N=1000: every loop-local
# instance runs `__del__`, resurrects, remains usable, and is freed once on the
# final drain.
import gc

N = 1000
box = []


class R:
    def __del__(self):
        box.append(self)


def run():
    i = 0
    while i < N:
        x = R()
        del x  # warm IC fast path; previously SIGSEGV past ~10 iterations
        i = i + 1
    gc.collect()
    print("resurrected", len(box))
    # Drain: each instance already ran __del__ once, so the final drop frees it
    # (it does NOT re-run __del__). End leak-clean.
    freed = 0
    while box:
        box.pop()
        freed = freed + 1
    gc.collect()
    print("freed", freed)
    print("box_empty", len(box) == 0)


run()
print("done")
