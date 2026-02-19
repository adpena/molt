"""Binary search helpers for Molt (intrinsic-backed)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_BISECT_LEFT = _require_intrinsic("molt_bisect_left", globals())
_MOLT_BISECT_RIGHT = _require_intrinsic("molt_bisect_right", globals())
_MOLT_BISECT_INSORT_LEFT = _require_intrinsic("molt_bisect_insort_left", globals())
_MOLT_BISECT_INSORT_RIGHT = _require_intrinsic("molt_bisect_insort_right", globals())

__all__ = [
    "bisect_left",
    "bisect_right",
    "bisect",
    "insort_left",
    "insort_right",
    "insort",
]


def bisect_left(a, x, lo=0, hi=None, *, key=None):
    return _MOLT_BISECT_LEFT(a, x, lo, hi, key)


def bisect_right(a, x, lo=0, hi=None, *, key=None):
    return _MOLT_BISECT_RIGHT(a, x, lo, hi, key)


def insort_left(a, x, lo=0, hi=None, *, key=None):
    _MOLT_BISECT_INSORT_LEFT(a, x, lo, hi, key)


def insort_right(a, x, lo=0, hi=None, *, key=None):
    _MOLT_BISECT_INSORT_RIGHT(a, x, lo, hi, key)


bisect = bisect_right
insort = insort_right
