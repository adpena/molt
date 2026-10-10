"""Compatibility surface for CPython `_posixsubprocess`."""

from _intrinsics import require_intrinsic as _require_intrinsic


fork_exec = _require_intrinsic("molt_process_spawn")

__all__ = ["fork_exec"]


globals().pop("_require_intrinsic", None)
