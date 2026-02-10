"""Concurrent utilities for Molt (subset)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_thread_submit", globals())
_require_intrinsic("molt_thread_spawn", globals())

from . import futures as futures  # noqa: E402

__all__ = ["futures"]
