"""Purpose: differential coverage for OR-pattern binding consistency."""

value = (1, 2)
try:
    match value:
        case (a, b) | (a, c):
            print("or", a, b, c)
        case _:
            print("or", "miss")
except Exception as exc:
    print("or", type(exc).__name__)
