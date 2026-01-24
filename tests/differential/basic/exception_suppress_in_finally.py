"""Purpose: differential coverage for raise from None inside finally."""

try:
    try:
        raise KeyError("inner")
    finally:
        raise ValueError("outer") from None
except Exception as exc:
    print("cause", exc.__cause__)
    print("context", exc.__context__)
    print("suppress", exc.__suppress_context__)
