"""Public API surface shim for ``dbm.dumb``."""

from __future__ import annotations


from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


class error(Exception):
    pass


def open(file: str, flag: str = "r", mode: int = 0o666):
    del file, flag, mode
    raise error("dbm.dumb backend is not implemented in Molt yet")
