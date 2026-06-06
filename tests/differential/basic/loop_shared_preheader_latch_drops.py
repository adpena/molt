# Regression: structured-loop reconstruction must decline a multi-entry region
# (round-9, the native DropInsertion-activation CFG/label blocker).
#
# `lower_to_simple_ir` reconstructs a `while`/`for` loop as one linear
# `loop_start … loop_continue/loop_break … loop_end` sequence and emits several
# of the region's interior blocks INLINE without their own `label` op (the cond /
# header-chain / guard-chain blocks, the first body block, and any body block
# whose terminator is the back-edge or the loop-exit edge). That is sound only
# when the region is single-entry — the loop HEADER is the unique block reachable
# from outside the region (natural-loop reducibility).
#
# This nested `while`-with-`for`-with-`break` shape, once the RC drop-insertion
# terminal phase reshapes the back-edges (critical-edge splits + retain
# placement), produces a SHARED pre-header/latch block: `entry → P → header`
# where the loop's back-edge also funnels `latch → P → header`. `P` is pulled
# into the body DFS (it is the back-edge's source-side block) yet still carries
# the external entry edge from `entry`. The old guard only checked the
# cond/header-chain/body-entry blocks for external predecessors, so it missed `P`,
# merged away `P`'s label, and left the entry's `jump P` dangling — a native
# Cranelift `label_blocks[&target]` "no entry found for key" panic, a WASM
# "unknown jump label" miscompile, and an
# "TIR roundtrip emitted invalid labels" warning on the same SimpleIR.
#
# This is a verbatim reduction of `typing._typing_strip_wrapping_parens`, the
# function the native drop flip first panicked on. It must run byte-identically
# to CPython on every backend (native / WASM / LLVM), with and without RC drops.


def strip_wrapping_parens(expr: str) -> str:
    text = expr.strip()
    while text.startswith("(") and text.endswith(")"):
        depth = 0
        balanced = True
        for idx, ch in enumerate(text):
            if ch == "(":
                depth += 1
            elif ch == ")":
                depth -= 1
                if depth == 0 and idx != len(text) - 1:
                    balanced = False
                    break
        if not balanced or depth != 0:
            break
        text = text[1:-1].strip()
    return text


# A second nested-loop-with-break shape that builds and discards a heap temporary
# (the str slice) on every back-edge — the loop-carried dead-at-back-edge value
# the drop phase splits a critical edge for.
def first_token(text: str) -> str:
    cleaned = text.strip()
    out = ""
    while cleaned:
        head = ""
        for ch in cleaned:
            if ch == " ":
                break
            head = head + ch
        if head:
            out = head
            break
        cleaned = cleaned[1:]
    return out


def main() -> None:
    print(strip_wrapping_parens("((a))"))
    print(strip_wrapping_parens("(a)(b)"))
    print(strip_wrapping_parens("  (nested(call))  "))
    print(strip_wrapping_parens("plain"))
    print(strip_wrapping_parens("(((deep)))"))
    print(strip_wrapping_parens("(unbalanced"))
    print(repr(first_token("   hello world  ")))
    print(repr(first_token("   ")))
    print(repr(first_token("oneword")))


main()
