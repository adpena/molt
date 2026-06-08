# Council #59 regression matrix, case 1/10: resurrect_once_N1.
#
# A no-`__init__`, `__del__`-resurrecting class instantiated ONCE. The class
# defines `__del__` but no `__init__`, so the resolved+cached constructor init
# is the inherited `object.__init__`, whose stored `fn_ptr` is a
# RUNTIME_CALLABLE_KEY_BASE MARKER (`0xFFFF_FF00_0000_0004`), not a real code
# address. N=1 keeps the type-call inline cache COLD (the IC fast path was the
# crash site; cold means the slow, marker-decoding fixed-arity path runs), so
# this is the safe-baseline anchor: it must agree with N10/N1000 byte-for-byte.
#
# Lifecycle invariant (council §D): `__del__` runs EXACTLY ONCE at the first
# zero-transition; the object is RESURRECTED into a live container (alive again,
# header/payload valid); the later final drop destroys it WITHOUT a second
# `__del__`. No SIGSEGV, no leak.
import gc

box = []


class R:
    # No __init__ on purpose -> inherited object.__init__ (marker fn_ptr) is the
    # resolved+cached constructor init.
    def __del__(self):
        box.append(self)
        box[-1].tag = "revived"


def run():
    x = R()
    del x  # rc 0 -> __del__ -> resurrected into box (alive again)
    gc.collect()
    print("after_first box_len", len(box))
    # The resurrected instance is fully usable: read the attribute __del__ set.
    print("revived_tag", box[0].tag)
    # Final drop: object truly dies, __del__ does NOT run again.
    box.clear()
    gc.collect()
    print("after_final box_len", len(box))


run()
print("done")
