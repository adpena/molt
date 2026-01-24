"""Purpose: differential coverage for multiple except* ordering with rethrow."""

try:
    raise ExceptionGroup("eg", [ValueError("a"), TypeError("b"), ValueError("c")])
except* ValueError as exc:
    print("value", [str(e) for e in exc.exceptions])
    raise
except* TypeError as exc:
    print("type", [str(e) for e in exc.exceptions])
