"""Purpose: differential coverage for bitwise int."""

print(f"int_and:{5 & 3}")
print(f"int_or:{5 | 2}")
print(f"int_xor:{5 ^ 3}")
print(f"int_invert:{~5}")
print(f"int_lshift:{5 << 3}")
print(f"int_rshift:{5 >> 1}")

print(f"bool_and:{True & False}")
print(f"bool_or:{True | False}")
print(f"bool_xor:{True ^ False}")
try:
    print(f"bool_invert:{~True}")
except TypeError as exc:
    # Keep later integer cases live on versions which reject Boolean inversion.
    print(f"bool_invert_err:{exc}")

for label, result in (
    ("bool_add", True + True),
    ("bool_neg", -True),
    ("bool_pos", +False),
    ("bool_shift", True << True),
    ("mixed_bitwise", True | 4),
    ("bytes_concat", b"left" + b"right"),
    ("bytes_repeat_bool", b"yes" * True),
    ("bytes_repeat_int", 3 * b"x"),
    ("text_repeat_bool", False * "unused"),
):
    print(label, type(result).__name__, result, result == result)

integer_accumulator = True
integer_accumulator += True
print("bool_inplace_add", type(integer_accumulator).__name__, integer_accumulator)
sequence_accumulator = b"left"
sequence_accumulator += b"right"
sequence_accumulator *= True
print("bytes_inplace", type(sequence_accumulator).__name__, sequence_accumulator)

big = 1 << 70
print(f"big_lshift:{big}")
print(f"big_rshift:{big >> 65}")
print(f"big_xor:{(big ^ (big >> 3)) >> 60}")
print("big_arithmetic_type", type(big + True).__name__, (big + True) > big)

try:
    discarded = (10**400) + 0.5
except OverflowError as exc:
    print("mixed_float_overflow", type(exc).__name__)

try:
    _ = 1 << -1
except ValueError as exc:
    print(f"neg_shift_err:{exc}")
