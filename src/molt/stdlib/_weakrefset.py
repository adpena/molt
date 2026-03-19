"""_weakrefset shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from weakref import WeakSet

_MOLT_WEAKSET_LEN = _require_intrinsic("molt_weakset_len")


__all__ = ["WeakSet"]

del _MOLT_WEAKSET_LEN

globals().pop("_require_intrinsic", None)
