"""Minimal encodings package for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from . import aliases as aliases

_require_intrinsic("molt_stdlib_probe", globals())
_require_intrinsic("molt_encodings_aliases_map", globals())

__all__ = ["aliases"]
