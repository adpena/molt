"""Purpose: differential coverage for except* rethrow ordering across branches."""

try:
    raise ExceptionGroup("eg", [ValueError("a"), TypeError("b"), KeyError("c")])
except* ValueError as exc:
    print("value", [type(e).__name__ for e in exc.exceptions])
    raise
except* TypeError as exc:
    print("type", [type(e).__name__ for e in exc.exceptions])
    raise
except* KeyError as exc:
    print("key", [type(e).__name__ for e in exc.exceptions])
