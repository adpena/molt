"""Differential coverage for counted-range sum(generator/listcomp) lowering.

`sum(<elt> for x in range(...))` (and the eager listcomp form) lowers the loop
through the counted-index shape a top-level `for x in range(...)` loop uses, so
`x` and the accumulator raw-lane. Every printed value must match CPython for the
full span of `range` semantics: positive/negative/zero step, empty ranges,
start/stop/step, large 64-bit sums, and the sum identity/empty result type.
"""


def main() -> None:
    # Positive ranges (pure counted lane for literal bounds).
    print("sq10", sum(x * x for x in range(10)))
    print("id1_11", sum(x for x in range(1, 11)))
    print("step2", sum(x * x for x in range(0, 20, 2)))
    print("step3", sum(x for x in range(5, 50, 3)))

    # Empty ranges -> int 0 (sum identity), including wrong-direction steps.
    print(
        "empty0", sum(x * x for x in range(0)), type(sum(x for x in range(0))).__name__
    )
    print("empty_eq", sum(x for x in range(5, 5)))
    print("empty_fwd", sum(x for x in range(10, 0)))
    print("empty_neg", sum(x for x in range(0, 10, -1)))

    # Negative step (generic range lane).
    print("negstep", sum(x for x in range(10, 0, -1)))
    print("negstep3", sum(x * x for x in range(20, 0, -3)))

    # Negative start/stop.
    print("negspan", sum(x for x in range(-5, 5)))
    print("negrange", sum(x for x in range(-10, -1)))

    # Float element: nonempty -> float, empty -> int 0.
    print("fnonempty", sum(x * 1.5 for x in range(5)))
    print(
        "fempty",
        sum(x * 1.5 for x in range(0)),
        type(sum(x / 2 for x in range(0))).__name__,
    )
    print("fdiv", sum(x / 2 for x in range(6)))

    # Eager listcomp form (identical semantics; consumed only by sum).
    print("lc_sq10", sum([x * x for x in range(10)]))
    print("lc_empty", sum([x for x in range(0)]))

    # Bool element sums as int.
    print("boolelt", sum(x > 5 for x in range(10)))

    # Large 64-bit sums (raw-lane must not truncate at 32/47 bits).
    print("large_id", sum(x for x in range(100000)))
    print("large_sq", sum(x * x for x in range(3000)))

    # Dynamic bounds through function parameters (generic range lane).
    def dyn(n: int, s: int, e: int, st: int) -> tuple[int, int, int]:
        return (
            sum(x * x for x in range(n)),
            sum(x for x in range(s, e)),
            sum(x for x in range(s, e, st)),
        )

    print("dyn", dyn(7, 2, 9, 2))
    print("dyn_empty", dyn(0, 5, 5, 1))

    # Element referencing an outer variable.
    k = 4
    print("outer", sum(x * k for x in range(k)))

    # Two independent sum-genexprs in one function.
    print("two", (sum(i for i in range(6)), sum(j * j for j in range(6))))

    # Zero step must raise ValueError, exactly as CPython range() does.
    try:
        print("step0", sum(x for x in range(0, 10, 0)))
    except ValueError as exc:
        print("step0_valueerror", str(exc))


# Module-scope sum-genexpr (molt_main lowering path).
MODULE_SUM = sum(x * x for x in range(12))


if __name__ == "__main__":
    print("module", MODULE_SUM)
    main()
