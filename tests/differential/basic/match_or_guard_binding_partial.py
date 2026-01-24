"""Purpose: differential coverage for OR-pattern guard binding without partials."""

value = (1, 2)
try:
    match value:
        case (a, b) | (c, d) if a == 1:
            print("match", a, b)
        case _:
            print("match", "miss")
    print("names", "a" in locals(), "c" in locals())
except Exception as exc:
    print("err", type(exc).__name__)
