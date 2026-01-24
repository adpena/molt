"""Purpose: differential coverage for nested ExceptionGroup splitting."""

try:
    raise ExceptionGroup(
        "eg",
        [
            ExceptionGroup("inner", [ValueError("a"), TypeError("b")]),
            KeyError("k"),
        ],
    )
except* ValueError as exc:
    print("value", [type(e).__name__ for e in exc.exceptions])
except* TypeError as exc:
    print("type", [type(e).__name__ for e in exc.exceptions])
except* KeyError as exc:
    print("key", [type(e).__name__ for e in exc.exceptions])
