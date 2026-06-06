# Regression: a `while True:` loop whose ONLY exit is an inner `if …: break`
# (round-10, the native DropInsertion-activation reachable-empty-block blocker).
#
# `lower_to_simple_ir` reconstructs a structured loop as one linear
# `loop_start … loop_break_if_X … loop_continue … loop_end` sequence. Which
# cond-block successor is the loop BODY (continue) versus the loop EXIT
# (after_block) was derived from the pre-TIR `loop_break_kinds` polarity hint.
# molt double-roundtrips through TIR (per-function pipeline → SimpleIR → relift
# for the RC drop module phase → SimpleIR), and the SSA terminator builder plus
# the drop phase's critical-edge reshaping can flip which side of the
# cond-block's `CondBranch` is the back-edge body. Trusting the stale hint then
# SWAPPED body_entry/exit_block — the EXIT (a `return` block) became the loop
# body, the back-edge CONTINUE became the exit. Native `loop_start` materializes
# an `after_block`; the swapped `loop_break_if_*` marks it reachable and jumps to
# it from the break-cleanup edge, but the matching `loop_end` (which would fill
# it) is never emitted for the degenerate shape, leaving a reachable-but-empty
# block that Cranelift's `unreachable_code` pass rejects (an
# `Option::unwrap() on None` panic deep in the backend). The historical "cranelift
# unreachable_code.rs:29 panic on double-break loops" is the same class.
#
# Fix: derive the body/exit polarity from the CFG (reducibility — the body is the
# cond successor from which the loop header is reachable), independent of the
# stale hint. Must run byte-identically to CPython on every backend
# (native / WASM / LLVM), with and without RC drops.


# Single inner break — the loop exit is the break, not the header condition.
# The loop-carried `s` (a heap str accumulator) is live across the break edge.
def concat_break(limit: int) -> int:
    s = ""
    i = 0
    while True:
        s = s + "z"
        i = i + 1
        if i >= limit:
            break
    return len(s)


# Two inner breaks (the "double-break" shape) — a second, never-taken break still
# splits the loop exit into multiple edges into the after_block.
def double_break(limit: int) -> int:
    s = ""
    i = 0
    while True:
        s = s + "z"
        i = i + 1
        if i >= limit:
            break
        if i == 999999:  # never taken, but adds a second break edge
            break
    return len(s)


# A break that carries a computed value out of the loop and uses it afterwards.
def sum_until(threshold: int) -> int:
    total = 0
    n = 0
    while True:
        n = n + 1
        total = total + n
        if total >= threshold:
            break
    return total + n


# `while True:` with the break guarded behind a non-trivial heap-valued condition,
# resetting the accumulator each call so a per-iteration leak would grow RSS.
def build_word(target: str) -> str:
    out = ""
    ch = "a"
    while True:
        out = out + ch
        if out == target:
            break
        if len(out) > len(target):
            break
    return out


def main() -> None:
    print(concat_break(1))
    print(concat_break(50))
    print(double_break(1))
    print(double_break(50))
    print(sum_until(1))
    print(sum_until(100))
    print(repr(build_word("aaaa")))
    print(repr(build_word("aaaaaaa")))


main()
