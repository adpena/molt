"""_weakrefset shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


_require_intrinsic("molt_stdlib_probe", globals())

from weakref import WeakSet

__all__ = ["WeakSet"]
