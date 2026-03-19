"""Intrinsic-backed ``dbm`` package for Molt.

Delegates to ``dbm.dumb`` as the default (and only) backend.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()

from dbm.dumb import error as _dumb_error

__all__ = ["error", "open", "whichdb"]

error = (_dumb_error, OSError)


def whichdb(filename: str) -> str | None:
    """Return the type of database, always 'dbm.dumb' in Molt."""
    import os

    if os.path.exists(filename + ".dir"):
        return "dbm.dumb"
    return None


def open(file: str, flag: str = "c", mode: int = 0o666) -> object:
    """Open a DBM database. Uses dbm.dumb backend."""
    import dbm.dumb

    return dbm.dumb.open(file, flag, mode)

globals().pop("_require_intrinsic", None)
