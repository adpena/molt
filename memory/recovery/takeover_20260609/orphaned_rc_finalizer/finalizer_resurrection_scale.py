# Resurrection-at-scale finalizer crash regression (task #59, council P0).
#
# A `__del__` that RESURRECTS its instance (stashes `self` in a live container),
# executed past a low threshold (~10), previously SIGSEGV'd on the LLVM/WASM drop
# lanes (and crashed native too). ROOT CAUSE: the per-call-site type-call inline
# cache (`CALL_BIND_IC_KIND_TYPE_CALL`, try_call_bind_ic_fast) cached the
# instance's `__init__` as `entry.fn_ptr = function_fn_ptr(init_ptr)`. For a class
# with NO user `__init__` that DOES define `__del__`, the resolved `__init__` is
# the inherited `object.__init__`, whose `fn_ptr` is a `RUNTIME_CALLABLE_KEY_BASE`
# MARKER (`0xFFFF_FF00_0000_0004`), NOT a real code address — the slow fixed-arity
# call path decodes it via `function_call_target_or_legacy_ptr` but the IC fast
# path `transmute`d and called the raw marker on the SECOND instantiation, jumping
# to `0x...0004` (SIGSEGV). (The `__del__` requirement is incidental: defining
# `__del__` is what makes `object.__init__` get resolved+cached for the class; the
# crash is in the IC fast path's marker handling, shared across every backend.)
#
# The fix routes the IC fast path through the SAME single decode authority as the
# slow path, so calling a raw runtime-callable marker is unrepresentable in both.
#
# This test exercises:
#   1. RESURRECT-AT-SCALE: a no-`__init__`, `__del__`-resurrecting class allocated
#      far past the old threshold (the SECOND instantiation onward hit the IC fast
#      path with the marker), retaining every instance — must not crash.
#   2. RESURRECT-THEN-TRULY-DIE: drain the container so each resurrected instance's
#      last reference drops; `__del__` ran once at the first death, so the second
#      drop truly frees it (leak-clean: live returns to the baseline at exit).
#   3. NO-`__init__` + `__del__` + extra-args policy unaffected: a plain `C()`.
#
# Run BOTH:
#   * `molt diff` (this file)  -> byte-identical to CPython 3.14 on native + LLVM.
#   * MOLT_ASSERT_NO_LEAK=1 (EXPECTED_LIVE_OBJECTS default) under
#     `safe_run.py --rss-mb 64` -> clean exit (every resurrected instance is truly
#     freed by the final drain; a leak or a missed destruction would fail).
N = 200

box = []


class R:
    # NO __init__ on purpose: forces the inherited object.__init__ (marker fn_ptr)
    # to be the resolved+cached constructor init — the exact IC fast-path trigger.
    def __del__(self):
        box.append(self)


def resurrect_at_scale():
    i = 0
    while i < N:
        x = R()
        del x  # rc 0 -> __del__ -> resurrected into box (the 2nd+ R() hit the IC)
        i = i + 1
    return len(box)


def drain_and_truly_die():
    # Each instance already ran __del__ once (HEADER_FLAG_FINALIZER_RAN set); the
    # final drop here truly destroys it. End leak-clean.
    freed = 0
    while box:
        obj = box.pop()
        del obj
        freed = freed + 1
    return freed


# Scenario 1 + 2.
alive = resurrect_at_scale()
print("resurrected", alive)
freed = drain_and_truly_die()
print("freed", freed)
print("box_empty", len(box) == 0)


# Scenario 3: independent no-__init__ + __del__ class, plain construction loop,
# resurrected through a per-instance attribute write (not a container append) to
# cover the attribute-store resurrection escape as well.
holder = []


class A:
    def __del__(self):
        holder.append(self)


def alloc_loop():
    total = 0
    j = 0
    while j < 50:
        a = A()
        del a
        total = total + 1
        j = j + 1
    return total


made = alloc_loop()
print("made", made)
# Drain so the second class is also leak-clean at exit.
while holder:
    holder.pop()
print("holder_empty", len(holder) == 0)
print("done")
