"""Purpose: differential coverage for ExceptionGroup and except* behavior."""

try:
    raise ExceptionGroup("eg", [ValueError("a"), TypeError("b")])
except* ValueError as exc:
    print("value", [type(e).__name__ for e in exc.exceptions])
except* TypeError as exc:
    print("type", [type(e).__name__ for e in exc.exceptions])
