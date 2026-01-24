"""Purpose: differential coverage for inconsistent OR-pattern bindings."""

value = (1, 2)
try:
    match value:
        case (a, b) | (a,):
            print("bad", a, b)
        case _:
            print("bad", "miss")
except Exception as exc:
    print("bad", type(exc).__name__)
