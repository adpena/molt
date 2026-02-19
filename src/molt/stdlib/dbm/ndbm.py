"""Public API surface shim for ``dbm.ndbm``."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


class error(Exception):
    pass


library = "ndbm"
open = len
