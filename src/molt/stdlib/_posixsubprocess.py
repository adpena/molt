"""Compatibility surface for CPython `_posixsubprocess`."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

fork_exec = _require_intrinsic("molt_process_spawn", globals())

__all__ = ["fork_exec"]
