"""Loop-invariant shift hoist (LICM proven-safe shift, #49).

`y = x << k` with `x` and `k` both loop-invariant is a pure, redundant
computation that should be hoisted once into the preheader. Shifts are
`pure_may_throw` (a negative count raises `ValueError`), so LICM correctly left
them out of the unconditional movable set — moving a possibly-raising op above
the loop guard would change WHEN the raise is observed.

When the shift count is value-range-proven in `[0, 63]` the `ValueError`
throw-condition is DISPROVEN at the hoist site, so the shift is provably nothrow
there and becomes LICM-hoistable. The accumulator `total += y` keeps `y`
loop-invariant; with the hoist, `x << k` is computed once instead of 30M times.

The masking keeps `total` in the inline window so the comparison HEAD-vs-post is
a clean shift-hoist measurement (no bigint accumulator noise).
"""


def compute(x: int, k: int) -> int:
    MASK = (1 << 40) - 1
    total = 0
    for _ in range(30_000_000):
        y = x << k          # loop-invariant; k proven in [0, 63] -> hoistable
        total = (total + y) & MASK
    return total


def main() -> None:
    print(compute(3, 12))


if __name__ == "__main__":
    main()
