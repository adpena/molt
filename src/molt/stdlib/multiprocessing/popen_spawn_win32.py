"""Windows-specific spawn backend (CPython-compatible import failure on non-Windows)."""

import os as _os


if _os.name != "nt":
    raise ModuleNotFoundError("No module named 'msvcrt'")

import msvcrt  # noqa: F401
