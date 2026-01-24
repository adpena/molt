"""Purpose: differential coverage for exception chaining basic."""

try:
    try:
        raise ValueError("inner")
    except Exception as exc:
        raise RuntimeError("outer") from exc
except Exception as exc:
    print(type(exc).__name__, type(exc.__cause__).__name__)
    print(type(exc.__context__).__name__)
    print(exc.__suppress_context__)

try:
    try:
        raise KeyError("key")
    except Exception:
        raise RuntimeError("outer2")
except Exception as exc:
    print(type(exc.__context__).__name__, exc.__cause__ is None)
