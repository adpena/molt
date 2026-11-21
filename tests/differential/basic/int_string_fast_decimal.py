import sys

cases = [
    "0",
    "42",
    "-42",
    "  +123  ",
    "9223372036854775807",
    "-9223372036854775808",
    "9223372036854775808",
    "\v -000000000000000000000003\f",
    "\u2003+１２_٣\u00a0",
]

for value in cases:
    print(value, int(value))

for value in ["", "abc", "+", "-", "a'b\n", "\ud800", b"hi'", b"\xff"]:
    try:
        int(value)
    except Exception as exc:
        print(repr(value), type(exc).__name__, str(exc))

old_limit = sys.get_int_max_str_digits()
limit = sys.int_info.str_digits_check_threshold
try:
    sys.set_int_max_str_digits(limit)
    print("max zero padding", int("0" * (limit - 1) + "2"))
    for padded in ("0" * limit + "2", b"0" * limit + b"2"):
        try:
            print(int(padded))
        except ValueError as exc:
            print(str(exc))
finally:
    sys.set_int_max_str_digits(old_limit)
