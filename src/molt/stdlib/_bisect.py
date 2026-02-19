"""Intrinsic-backed bisect primitives for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


__all__ = [
    "bisect_left",
    "bisect_right",
    "insort_left",
    "insort_right",
]

# Keep `_bisect` aligned with CPython's C-extension shape by exposing the
# raw intrinsic callables directly.
bisect_left = _require_intrinsic("molt_bisect_left", globals())
bisect_right = _require_intrinsic("molt_bisect_right", globals())
insort_left = _require_intrinsic("molt_insort_left", globals())
insort_right = _require_intrinsic("molt_insort_right", globals())
