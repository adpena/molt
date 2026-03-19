"""Compatibility surface for CPython `_posixsubprocess`."""

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

fork_exec = _require_intrinsic("molt_process_spawn")

__all__ = ["fork_exec"]


globals().pop("_require_intrinsic", None)
