"""Purpose: differential coverage for raise from and __context__/__cause__."""

try:
    try:
        raise KeyError("inner")
    except Exception as exc:
        raise ValueError("outer") from exc
except Exception as exc:
    print("cause", type(exc.__cause__).__name__)
    print("context", type(exc.__context__).__name__)
    print("suppress", exc.__suppress_context__)

try:
    try:
        raise KeyError("inner")
    except Exception:
        raise ValueError("outer")
except Exception as exc:
    print("cause2", exc.__cause__)
    print("context2", type(exc.__context__).__name__)
    print("suppress2", exc.__suppress_context__)
