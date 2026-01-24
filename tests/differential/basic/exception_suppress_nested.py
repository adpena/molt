"""Purpose: differential coverage for nested raise from None suppression."""

try:
    try:
        raise KeyError("inner")
    except Exception:
        raise ValueError("mid") from None
except Exception as exc:
    print("mid", exc.__cause__, exc.__context__, exc.__suppress_context__)

try:
    try:
        raise KeyError("inner")
    except Exception as exc:
        raise ValueError("mid") from exc
except Exception as exc:
    print("mid2", type(exc.__cause__).__name__, type(exc.__context__).__name__, exc.__suppress_context__)
