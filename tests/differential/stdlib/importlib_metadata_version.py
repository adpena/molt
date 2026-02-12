"""Purpose: differential coverage for importlib metadata version."""

import importlib.metadata as md


try:
    print(md.version("pip"))
except Exception as exc:
    print(type(exc).__name__)
