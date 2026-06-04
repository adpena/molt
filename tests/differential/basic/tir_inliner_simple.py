"""TIR function inliner (E1 phases a+b) differential + perf regression.

`helper()` is a tiny exception-free, non-recursive, non-generator leaf that
returns a constant.  `a()` calls it in a hot loop and accumulates the result.
The inliner must splice `helper`'s body into `a` so the call boundary vanishes
and the constant flows into the loop body, leaving a tight integer accumulator
that beats CPython.  Output must be byte-identical to CPython.
"""


def helper() -> int:
    return 42


def a(n: int) -> int:
    total = 0
    i = 0
    while i < n:
        total += helper() + 1
        i += 1
    return total


def main() -> None:
    # 5,000,000 iterations of a tiny helper call — the call overhead is what
    # inlining removes.  43 per iteration.
    print(a(5_000_000))
    # Small deterministic checks so the differential harness compares an exact
    # value, not just the hot-loop sum.
    print(a(0))
    print(a(1))
    print(a(10))


main()
