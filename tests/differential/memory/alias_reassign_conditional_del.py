# RC drop-insertion over-release regression — alias-reassign with a conditional
# `del` (review-required variant).
#
# Combines the alias-then-reassign accumulator with a `del y` on a control-flow
# branch. The `del` makes the alias `y`'s lifetime END mid-iteration on one path
# but not the other, stressing the alias-GROUP live-out reasoning across blocks:
# the single owned reference shared by `x`/`y` must be released exactly once
# regardless of whether the `del` branch is taken, and never double-released when
# the loop-exit `return x` also consumes the (re-aliased) accumulator.
#
# Byte-identical to CPython on LLVM AND native; bounded RSS (no leak / no UAF).
def aliased_with_del(n):
    x = "s"
    i = 0
    while i < n:
        y = x
        x = y + str(i % 7)
        if i % 2 == 0:
            del y          # alias dies early on the even branch
        i = i + 1
    return x[-3:]


print(aliased_with_del(60))
