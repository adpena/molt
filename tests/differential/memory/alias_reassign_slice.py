# RC drop-insertion over-release regression — adversarial-review P0 #1.
#
# `y = x; x = y + str(i)` aliases the accumulator into `y`, then reassigns `x`;
# the result is finally SLICED with `[-5:]`. The crash had TWO compounding roots,
# both in the lowering-truth alias contract:
#
#   1. The string subscript `s[-5:]` lowered to `Copy[_original_kind="slice"]`,
#      which the LLVM backend SILENTLY passed through (returned operand 0 — the
#      source string — instead of a fresh slice). That made the slice result an
#      un-modeled no-incref alias of its source.
#   2. The drop pass treated the slice result as an independent owned value and
#      emitted `DecRef(slice)` AND `DecRef(source)` for what is, on the broken
#      backend, ONE object → double-free (LLVM printed freed memory `<object>`
#      then SIGABRT'd on the second dec_ref).
#
# Fix: `slice` (and the other value-producing `Copy` kinds) is now lowered
# explicitly in LLVM as a fresh owned object AND classified `FreshValue`; the
# alias view fails closed to "alias" for everything not proven to mint a fresh
# `+1`. Must be byte-identical to CPython on LLVM AND native.
def aliased_then_reassigned(n):
    x = "seed"
    i = 0
    while i < n:
        y = x            # alias of the accumulator phi
        x = y + str(i)   # x reassigned; old x dead
        i = i + 1
    return x


print(aliased_then_reassigned(100)[-5:])
