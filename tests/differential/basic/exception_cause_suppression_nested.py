"""Purpose: differential coverage for cause vs suppress in nested handlers."""

try:
    try:
        raise KeyError("inner")
    except Exception as exc:
        raise ValueError("mid") from exc
finally:
    try:
        raise RuntimeError("outer") from None
    except Exception as exc:
        print("outer", exc.__cause__, exc.__context__, exc.__suppress_context__)
