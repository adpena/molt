"""Purpose: differential coverage for except* rethrow flowing to later handlers."""

try:
    raise ExceptionGroup("eg", [ValueError("a"), TypeError("b"), KeyError("c")])
except* ValueError as exc:
    print("value", [str(e) for e in exc.exceptions])
    raise
except* TypeError as exc:
    print("type", [str(e) for e in exc.exceptions])
except* KeyError as exc:
    print("key", [str(e) for e in exc.exceptions])
