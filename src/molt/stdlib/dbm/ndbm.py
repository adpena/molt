"""Public API surface shim for ``dbm.ndbm``."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")


class error(Exception):
    pass


library = "ndbm"
open = len

globals().pop("_require_intrinsic", None)
