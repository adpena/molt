# Council #59 regression matrix, case 2/10: resurrect_once_N10.
#
# Same no-`__init__`, `__del__`-resurrecting class as N1, but instantiated N=10
# times. N=10 is the OLD crash threshold: by the second+ construction the
# per-call-site type-call inline cache is WARM, so the IC fast path
# (`try_call_bind_ic_fast`) fires. Before the fix, that path `transmute`d the raw
# `object.__init__` marker `fn_ptr` and jumped to `0xFFFF...04` -> SIGSEGV. The
# fix routes the IC fast path through the same required call-target authority as
# the slow fixed-arity path, so calling a raw runtime-callable marker is
# unrepresentable.
#
# Lifecycle invariant (council §D): each of the 10 instances runs `__del__`
# EXACTLY ONCE (resurrect), stays usable, and is destroyed once on the final
# drain. Byte-identical to N1/N1000.
#
# STATUS: native differential pass. The historical IC SIGSEGV and the later
# loop-body finalizer-drop gap are both fixed for this warm-call-site shape:
# the loop-local instances run `__del__`, resurrect, remain usable, and are
# freed once on the final drain.
import gc

box = []


class R:
    def __del__(self):
        box.append(self)
        box[-1].tag = "revived"


def run():
    i = 0
    while i < 10:
        x = R()
        del x  # 2nd+ iteration hits the WARM IC fast path (old crash site)
        i = i + 1
    gc.collect()
    print("after_first box_len", len(box))
    # Every resurrected instance is usable.
    tags = 0
    for obj in box:
        if obj.tag == "revived":
            tags = tags + 1
    print("revived_count", tags)
    box.clear()
    gc.collect()
    print("after_final box_len", len(box))


run()
print("done")
