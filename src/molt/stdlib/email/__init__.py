"""Intrinsic-backed email package surface for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

from . import message as message  # noqa: E402

__all__ = ["message"]
