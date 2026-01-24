"""Purpose: differential coverage for raise from None context suppression."""

try:
    try:
        raise KeyError("inner")
    except Exception:
        raise ValueError("outer") from None
except Exception as exc:
    print("cause", exc.__cause__)
    print("context", exc.__context__)
    print("suppress", exc.__suppress_context__)
