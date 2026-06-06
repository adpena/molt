"""Masked back-edge shift accumulator (value-range phi-narrowing recovery, #43).

`s = (s << 1) & MASK` is a loop-carried recurrence whose back-edge value is
re-bounded to `[0, MASK]` by the mask, INDEPENDENT of the phi `s`. Without
phi-range narrowing the value-range fixpoint leaves the header phi `s` at
FULL_I64, so the shift result `s << 1` is unproven, the raw-i64 lane is refused,
and every iteration boxes through `molt_lshift` (~0.635s / 30M iters measured).

With sound phi-range narrowing the phi is proven `[0, MASK]`, the shift result
fits the inline window with a `[0, 63]` count, the raw machine lane is restored,
and the loop runs as bare i64 arithmetic. The result is exact either way — this
benchmark guards the PERF, not correctness (correctness is in
tests/differential/basic/shift_overflow_matrix.py).
"""


def main() -> None:
    MASK = (1 << 32) - 1
    s = 1
    for _ in range(30_000_000):
        s = (s << 1) & MASK
    print(s)


if __name__ == "__main__":
    main()
