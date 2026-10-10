"""Intrinsic-backed ``dbm`` package for Molt.

Delegates to ``dbm.dumb`` as the default (and only) backend.
"""

from __future__ import annotations


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
