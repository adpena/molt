"""Purpose: regression for the coroutine/generator `_poll` label-roundtrip panic.

Compiling a `for`-loop-with-`yield` (a generator/coroutine `_poll` state
machine) produced a backend whose loop-CONDITION block is re-entered by an
explicit resume `jump` originating OUTSIDE the loop region (the state-dispatch
edge that resumes execution after a `yield`/`await` suspension).  The
structured-loop reconstruction in `tir::lower_to_simple` consumed that
condition block inline — dropping its label — while the external resume jump
still referenced it, tripping the
`TIR roundtrip emitted invalid labels for '..._poll'` assertion in the native
backend (`simple_backend.rs`).

The fix declines structured-loop reconstruction whenever an inline-consumed
block (the condition block, the header/guard chain, or the first body block)
has a predecessor outside the region, falling back to generic
label-preserving lowering.

These pure-generator shapes drive the exact `_poll` loop-condition-reentry
path end-to-end (no `asyncio.sleep` dependency), so the differential harness
both compiles them (the original panic site) and runs them to completion.
The equivalent `async for` shape over an async generator is the same bug
class and is covered by the TIR-level unit test
`loop_cond_with_external_reentry_keeps_label_no_dangling`.
"""


def gen_loop(n):
    total = 0
    for i in range(n):
        total += i
        yield total
    yield -1


def gen_try(n):
    total = 0
    for i in range(n):
        try:
            if i % 2 == 0:
                raise KeyError(i)
            total += i
        except KeyError:
            total -= 1
        yield total


def gen_nested(rows, cols):
    for r in range(rows):
        acc = 0
        for c in range(cols):
            acc += r * cols + c
            yield (r, c, acc)


print(list(gen_loop(4)))
print(list(gen_try(5)))
print(list(gen_nested(2, 3)))
