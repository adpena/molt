"""Purpose: differential coverage for importlib metadata entry points."""

import importlib.metadata as md


try:
    eps = md.entry_points()
    print(hasattr(eps, "select"), len(eps))
except Exception as exc:
    print(type(exc).__name__)
