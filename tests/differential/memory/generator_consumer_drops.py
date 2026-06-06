# RC drop-insertion regression — generator-CONSUMER yielded-element ownership
# (adversarial-review P0 #2(b)).
#
# The yielded element is the VALUE result of `IterNextUnboxed` (results[0]). The
# runtime writes that value-out slot ONLY on the not-done branch; on the
# `done=true` exhaustion path it leaves the slot UNINITIALIZED (stale stack
# garbage — every runtime `return done_true` skips `*value_out = …`). The drop
# pass previously placed an edge-dying `DecRef(element)` at the loop-exit
# successor, decreffing that stale pointer → use-after-free / segfault on
# `list(g())`, `"".join(g())`, and bare `for v in g():`.
#
# Fix: the `IterNextUnboxed` value result is now EXCLUDED from edge-dying drops
# (`drop_insertion`'s `iter_cond_value_results`). On the not-done body path the
# element is valid and released by the ordinary straight-line last-use rule (after
# the consumer increfs it into the list / joins it); on the exhaustion edge it is
# never touched. The generator object itself is owned (`iter()` increfs it) and
# released once by the consumer.
#
# NOTE (out-of-scope blocker): the LLVM backend has a SEPARATE, PRE-EXISTING
# generator-codegen bug — bare `g(n)` creation segfaults on LLVM even with the
# drop pass disabled (`MOLT_DROPINS_OFF=1`) and on `list(...)`-free programs.
# This test is therefore byte-identical to CPython on NATIVE; on LLVM it is
# blocked by that independent codegen bug, NOT by drop insertion (the drop-pass
# placement verified correct via the MOLT_DEBUG_DROP dump: the bad exhaustion-edge
# DecRef of the element is gone).
def gen_strings(n):
    i = 0
    while i < n:
        yield "g" + str(i)
        i = i + 1


def consume_for():
    out = []
    for v in gen_strings(5):
        out.append(v)
    return out


def consume_list(n):
    return list(gen_strings(n))


def consume_join(n):
    return "".join(gen_strings(n))


r = consume_for()
print(len(r), r[0], r[-1])
print(len(consume_list(5)))
print(consume_join(4))
