"""Binary search helpers for Molt (intrinsic-backed)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

_bisect_left = _require_intrinsic("molt_bisect_left", globals())
_bisect_right = _require_intrinsic("molt_bisect_right", globals())
_insort_left = _require_intrinsic("molt_insort_left", globals())
_insort_right = _require_intrinsic("molt_insort_right", globals())

__all__ = [
    "bisect_left",
    "bisect_right",
    "bisect",
    "insort_left",
    "insort_right",
    "insort",
]


def bisect_left(a, x, lo=0, hi=None, *, key=None):
    return _bisect_left(a, x, lo, hi, key)


def bisect_right(a, x, lo=0, hi=None, *, key=None):
    return _bisect_right(a, x, lo, hi, key)


def insort_left(a, x, lo=0, hi=None, *, key=None) -> None:
    _insort_left(a, x, lo, hi, key)


def insort_right(a, x, lo=0, hi=None, *, key=None) -> None:
    _insort_right(a, x, lo, hi, key)


bisect = bisect_right
insort = insort_right
