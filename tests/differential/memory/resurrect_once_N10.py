# Council #59 regression matrix, case 2/10: resurrect_once_N10.
#
# Same no-`__init__`, `__del__`-resurrecting class as N1, but instantiated N=10
# times. N=10 is the OLD crash threshold: by the second+ construction the
# per-call-site type-call inline cache is WARM, so the IC fast path
# (`try_call_bind_ic_fast`) fires. Before the fix, that path `transmute`d the raw
# `object.__init__` marker `fn_ptr` and jumped to `0xFFFF...04` -> SIGSEGV. The
# fix routes the IC fast path through the SAME single decode authority
# (`function_call_target_or_legacy_ptr`) as the slow fixed-arity path, so calling
# a raw runtime-callable marker is unrepresentable.
#
# Lifecycle invariant (council §D): each of the 10 instances runs `__del__`
# EXACTLY ONCE (resurrect), stays usable, and is destroyed once on the final
# drain. Byte-identical to N1/N1000.
#
# STATUS: the IC SIGSEGV is FIXED (this no longer crashes), but a SEPARATE,
# pre-existing defect — the loop-body finalizer-drop gap (task #58 / design-27
# ownership lattice, parallel-session-owned) — means a construct-and-`del` inside
# a `while`/`for` loop never lowers a per-iteration finalizer-aware DecRef, so
# `__del__` does not fire for the loop-local instances. This is NOT the IC bug:
# the IDENTICAL class constructed+del'd STRAIGHT-LINE (cold OR warm IC) finalizes
# correctly (see resurrect_once_N1 and the straight-line IC-warm probe). Marked
# xfail against #58 until loop-body drop placement lands; it auto-flips to a loud
# xpass-failure the moment the loop-drop arc is fixed.
# NOTE: byte-identical on the LLVM backend (which lowers loop-body finalizer
# drops correctly) — the gap is NATIVE-Cranelift-specific drop lowering.
# MOLT_META: xfail=molt xfail_reason=#58-loop-body-finalizer-drop-gap-NATIVE-cranelift-only-not-the-IC-fix
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
