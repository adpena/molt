"""Purpose: differential coverage for number-format digit grouping combined
with sign-aware '0' fill (the '=' alignment path).

Regression for a silent-wrong-answer bug where the thousands/underscore
separator was dropped from the zero-padding region, e.g. format(42, "08,d")
produced "00000042" instead of CPython's "0,000,042". Exercises int / float /
percent / hex / octal / binary, every sign, bigint & bool, all three entry
points (format(), f-strings, str.format()), the explicit-alignment cases that
must NOT group their fill, and the CPython grouping/type validation errors.
"""


def show_format(label, value, spec):
    try:
        print(f"{label}: {format(value, spec)!r}")
    except Exception as exc:
        print(f"{label}: {type(exc).__name__}: {exc}")

# The '0' fill flag (sign-aware '=' alignment) interleaves grouping across the
# zero-fill region, so the field can legitimately exceed the requested width.
GROUPED_ZERO_FILL = [
    (42, "08,d"),
    (-42, "08,d"),
    (1234567, "015,d"),
    (255, "012_x"),
    (255, "#012_x"),
    (1234.5, "012,.2f"),
    (0.5, "010,.1%"),
    (0.5, "010,.0%"),
    # sign interaction ('+', ' ', '-')
    (1234.5, "+012,.2f"),
    (1234.5, " 012,.2f"),
    (-1234.5, "012,.2f"),
    (1000000, "+020,d"),
    (42, "+08,d"),
    (42, " 08,d"),
    (-42, "+08,d"),
    # underscore decimal (group 3) and b/o/x bases (group 4) with zero fill
    (1234567, "020_d"),
    (1000000, "020_d"),
    (0o777777, "012_o"),
    (255, "012_b"),
    (0xFFFFFF, "012_x"),
    (255, "#014_x"),
    # exponential notation groups its zero fill, mantissa left untouched
    (1.5, "020,e"),
    (12345.678, "022,.3e"),
    (1.5e20, "028,.3e"),
    # edge widths where the grouped field jumps past the requested width
    (0, "05,d"),
    (0, "05,.0f"),
    (1, "04,d"),
    (12, "03,d"),
    (12, "01,d"),
    (123, "04,d"),
    (7, "06,d"),
    (7, "07,d"),
    (7, "08,d"),
    # explicit '=' alignment with an explicit '0' fill char also groups
    (42, "0=8,d"),
    # bigint and bool
    (10**30, "045,d"),
    (True, "08,d"),
    (False, "08,d"),
    (True, "08,"),
    (False, "08,"),
]
for value, spec in GROUPED_ZERO_FILL:
    print(f"{spec!r:>12} -> {format(value, spec)!r}")

# Natural grouping (no zero fill): decimal groups by 3; b/o/x bases by 4.
NATURAL = [
    (0o777777, "_o"),
    (255, "_b"),
    (0xFFFFFF, "_x"),
    (1234567, "_d"),
    (1234567, ","),
    (1234567, "_"),
    (True, ","),
    (False, "_"),
    (1.5, ",e"),
    (1234567.5, ",.2f"),
    (1234567.5, "_.2f"),
]
for value, spec in NATURAL:
    print(f"{spec!r:>12} -> {format(value, spec)!r}")

# Padding that must NOT be grouped: any non-'=' alignment, or '=' with a
# non-'0' fill char. Only the '0' flag / explicit '0='-with-'0' groups.
NO_GROUP_PAD = [
    (42, "0>8,d"),
    (42, "0<8,d"),
    (42, "0^8,d"),
    (42, "*=8,d"),
    (42, " =8,d"),
    (42, "x=10,d"),
]
for value, spec in NO_GROUP_PAD:
    print(f"{spec!r:>12} -> {format(value, spec)!r}")

# Non-grouped paths must be unaffected by the fix.
UNAFFECTED = [
    (255, "c"),
    (42, "08d"),
    (42, "8d"),
    (3.14159, "010.2f"),
    (255, "08x"),
    (255, "#08x"),
    (1234567, "d"),
    (1234.5, ".2f"),
]
for value, spec in UNAFFECTED:
    print(f"{spec!r:>12} -> {format(value, spec)!r}")

# Entry-point parity: format(), f-strings, and str.format() agree.
print(f"{42:08,d}")
print("{:08,d}".format(42))
print(f"{1234567:015,d}")
print("{:012,.2f}".format(1234.5))
print(f"{255:#012_x}")
print(f"{0.5:010,.1%}")
print(f"{-42:08,d}")
print("{0:+012,.2f}".format(1234.5))

show_format("bool_empty", True, "")
show_format("bool_width", True, ">8")
show_format("bool_string_type_error", True, "s")

show_format("str_zero_flag_left_pad", "x", "08s")
show_format("str_explicit_zero_right_pad", "x", "0>8s")
show_format("str_explicit_equal_error", "x", "0=8s")
show_format("str_group_error", "x", ",s")
show_format("str_space_sign_error", "x", " s")
show_format("str_sign_error", "x", "+s")
show_format("str_alt_error", "x", "#s")
show_format("str_numeric_type_error", "x", "d")

show_format("list_empty_default", [], "")
show_format("list_nonempty_spec_error", [], ">8")
show_format("none_nonempty_spec_error", None, ">8")
show_format("bytes_nonempty_spec_error", b"x", ">8")

show_format("char_sign_error", 65, "+c")
show_format("char_alt_error", 65, "#c")
show_format("char_precision_error", 65, ".2c")
show_format("unknown_int_type_error", 42, "q")
show_format("unknown_bool_type_error", True, "q")

# CPython rejects incompatible grouping/type pairings (message parity).
ERRORS = [
    (255, ",x"),
    (255, ",o"),
    (255, ",b"),
    (255, ",X"),
    (65, ",c"),
    (65, "_c"),
    (42, ",n"),
    (42, "_n"),
]
for value, spec in ERRORS:
    try:
        result = format(value, spec)
        print(f"{spec!r:>6} -> NO ERROR {result!r}")
    except ValueError as exc:
        print(f"{spec!r:>6} -> ValueError: {exc}")
