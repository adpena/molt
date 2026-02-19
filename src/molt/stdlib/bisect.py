"""Binary search helpers for Molt (intrinsic-backed)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import _bisect

# Mark this facade module as intrinsic-backed for stdlib gating.
_require_intrinsic("molt_bisect_left", globals())
_require_intrinsic("molt_bisect_right", globals())
_require_intrinsic("molt_insort_left", globals())
_require_intrinsic("molt_insort_right", globals())

__all__ = [
    "bisect_left",
    "bisect_right",
    "bisect",
    "insort_left",
    "insort_right",
    "insort",
]


def _coerce_index(value):
    if isinstance(value, int):
        return True, value, None
    try:
        idx = value.__index__()
    except AttributeError:
        return (
            False,
            0,
            f"'{type(value).__name__}' object cannot be interpreted as an integer",
        )
    if not isinstance(idx, int):
        return (
            False,
            0,
            f"__index__ returned non-int (type {type(idx).__name__})",
        )
    return True, idx, None


# Return errors for the caller to raise to avoid delayed exception propagation.
def _normalize_bounds(lo, hi, size):
    ok, lo_idx, err = _coerce_index(lo)
    if not ok:
        return 0, 0, TypeError(err)
    if lo_idx < 0:
        return 0, 0, ValueError("lo must be non-negative")
    if hi is None:
        return lo_idx, size, None
    ok, hi_idx, err = _coerce_index(hi)
    if not ok:
        return 0, 0, TypeError(err)
    if hi_idx > size:
        return 0, 0, IndexError("list index out of range")
    return lo_idx, hi_idx, None


def bisect_left(a, x, lo=0, hi=None, *, key=None):
    lo_idx, hi_idx, err = _normalize_bounds(lo, hi, len(a))
    if err is not None:
        raise err
    return _bisect.bisect_left(a, x, lo_idx, hi_idx, key)


def bisect_right(a, x, lo=0, hi=None, *, key=None):
    lo_idx, hi_idx, err = _normalize_bounds(lo, hi, len(a))
    if err is not None:
        raise err
    return _bisect.bisect_right(a, x, lo_idx, hi_idx, key)


def insort_left(a, x, lo=0, hi=None, *, key=None):
    lo_idx, hi_idx, err = _normalize_bounds(lo, hi, len(a))
    if err is not None:
        raise err
    _bisect.insort_left(a, x, lo_idx, hi_idx, key)


def insort_right(a, x, lo=0, hi=None, *, key=None):
    lo_idx, hi_idx, err = _normalize_bounds(lo, hi, len(a))
    if err is not None:
        raise err
    _bisect.insort_right(a, x, lo_idx, hi_idx, key)


bisect = bisect_right
insort = insort_right
