"""Purpose: differential coverage for ExceptionGroup reraising partial groups."""

try:
    raise ExceptionGroup("eg", [ValueError("a"), KeyError("b")])
except* ValueError as exc:
    print("value", [type(e).__name__ for e in exc.exceptions])
    raise
except* KeyError as exc:
    print("key", [type(e).__name__ for e in exc.exceptions])
