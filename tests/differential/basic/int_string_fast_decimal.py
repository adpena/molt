cases = [
    "0",
    "42",
    "-42",
    "  +123  ",
    "9223372036854775807",
    "-9223372036854775808",
    "9223372036854775808",
]

for value in cases:
    print(value, int(value))

for value in ["", "abc", "+", "-"]:
    try:
        int(value)
    except Exception as exc:
        print(value, type(exc).__name__, str(exc))
