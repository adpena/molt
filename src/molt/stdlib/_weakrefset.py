"""_weakrefset shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


from weakref import WeakSet

_require_intrinsic("molt_weakset_len", globals())


__all__ = ["WeakSet"]
