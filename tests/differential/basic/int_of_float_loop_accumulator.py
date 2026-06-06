# Regression: `int(float)` in a loop-carried integer accumulator (round-8).
#
# `int(t)` where `t: float` lowers to `Copy[int_from_obj](t)` — a fresh-value
# `Copy` whose result is a real `int`, NOT a transparent alias of its float
# operand. type_refine's old `Copy => operand_types.first()` rule type-aliased the
# result to the operand's `float`, flooding the integer accumulator chain (and its
# loop-carried / conditional-join phis) with a spurious `float` carrier. On the
# value-keyed lanes this produced a phi-edge representation mismatch (a raw i64
# value delivered into a float-typed phi slot); the shape below is the reduction
# of `os._seconds_float_to_sec_nsec` that exposed it as a native Cranelift
# `def_var` repr mismatch / LIR-verifier branch-repr divergence.
#
# Must stay byte-identical to CPython on every backend.


def sec_nsec(t: float) -> tuple[int, int]:
    # int(float) feeds an integer accumulator carried across a while loop with
    # an if/elif normalization — the exact join-phi shape that mis-typed.
    sec = int(t)
    frac_ns = int(round((t - sec) * 1_000_000_000.0))
    if frac_ns >= 1_000_000_000:
        sec += 1
        frac_ns -= 1_000_000_000
    elif frac_ns < 0:
        sec -= 1
        frac_ns += 1_000_000_000
    return sec, frac_ns


def loop_accumulate(t: float, n: int) -> int:
    total = 0
    outer = 0
    while outer < n:
        s = int(t)
        total += s
        if s < 0:
            total -= 1
        else:
            total += 1
        outer += 1
    return total


def mixed_int_float(t: float) -> float:
    # The integer `int(t)` result must NOT contaminate the genuinely-float
    # remainder: `t - int(t)` is float, `int(t) * 2` is int.
    base = int(t)
    remainder = t - base
    doubled = base * 2
    return remainder + doubled


def main() -> None:
    for t in (3.7, -2.3, 0.0, 1.0, 1234.5678, -0.0000001):
        print(sec_nsec(t))
    print(loop_accumulate(3.7, 5))
    print(loop_accumulate(-2.9, 4))
    print(loop_accumulate(0.4, 7))
    for t in (3.7, -2.3, 10.99):
        print(round(mixed_int_float(t), 6))


main()
