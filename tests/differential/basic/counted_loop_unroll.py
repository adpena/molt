# Constant-trip counted loops — the canonical shape the L4 counted-loop
# contract recognizes so loop_unroll can fully unroll them. Every value must be
# byte-identical to CPython, both for small trip counts (unrolled) and large
# (left rolled), with accumulators that exceed the inline-int range (bug #15:
# an unbounded/large accumulator must stay BigInt-correct, never carried as a
# wrapped raw i64).


def small_sum():
    total = 0
    for i in range(8):
        total += i
    return total


def empty_loop():
    total = 0
    for i in range(0):
        total += i
    return total


def single_iter():
    total = 0
    for i in range(1):
        total += i
    return total


def stepped():
    total = 0
    for i in range(2, 10, 2):
        total += i
    return total


def large_trip():
    # Trip count 100 > unroll cap → recognized but left rolled; must still be
    # correct (and exercises the recognizer's "above cap" refusal-to-unroll).
    total = 0
    for i in range(100):
        total += i
    return total


def accumulator_past_2_47():
    # Final accumulator far exceeds 2**47 (the NaN-box inline-int limit): it
    # must remain a BigInt-correct value, never a wrapped raw i64.
    total = 0
    for i in range(8):
        total += (1 << 50) + i
    return total


def accumulator_past_2_63():
    # Final accumulator exceeds 2**63 (the raw i64 wrap point): BigInt only.
    total = 0
    for i in range(8):
        total += (1 << 62) + i
    return total


def break_in_body():
    total = 0
    for i in range(8):
        if i == 4:
            break
        total += i
    return total


def continue_in_body():
    total = 0
    for i in range(8):
        if i % 2 == 0:
            continue
        total += i
    return total


def nested_counted():
    total = 0
    for i in range(4):
        for j in range(3):
            total += i * j
    return total


# NOTE: a `range(hi, lo, -1)` negative-step counted loop is intentionally NOT
# exercised here as a top-level differential — it triggers a PRE-EXISTING,
# loop-unroll-INDEPENDENT native dev-profile codegen bug ("block N cannot be
# empty" from the Cranelift verifier) that reproduces even when loop_unroll does
# NOT fire on the loop. The negative-step UNROLL path itself is covered by the
# `unrolls_real_multiarg_header_negative_step` Rust unit test. See the baton.
print(small_sum())
print(empty_loop())
print(single_iter())
print(stepped())
print(large_trip())
print(accumulator_past_2_47())
print(accumulator_past_2_63())
print(break_in_body())
print(continue_in_body())
print(nested_counted())
