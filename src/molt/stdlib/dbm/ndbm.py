"""Public API surface shim for ``dbm.ndbm``."""

from __future__ import annotations


class error(Exception):
    pass


library = "ndbm"
open = len
