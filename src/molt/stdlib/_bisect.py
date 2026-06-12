"""Intrinsic-backed _bisect core.

The runtime intrinsics (`molt_bisect_*`) take all five arguments positionally
(`a, x, lo, hi, key`) with no defaults. CPython's `_bisect` C functions expose
the signature `(a, x, lo=0, hi=None, *, key=None)` with `key` keyword-only, so
these thin wrappers supply those defaults and present the exact public shape.
"""

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_BISECT_LEFT = _require_intrinsic("molt_bisect_left")
_MOLT_BISECT_RIGHT = _require_intrinsic("molt_bisect_right")
_MOLT_BISECT_INSORT_LEFT = _require_intrinsic("molt_bisect_insort_left")
_MOLT_BISECT_INSORT_RIGHT = _require_intrinsic("molt_bisect_insort_right")


def bisect_left(a, x, lo=0, hi=None, *, key=None):
    return _MOLT_BISECT_LEFT(a, x, lo, hi, key)


def bisect_right(a, x, lo=0, hi=None, *, key=None):
    return _MOLT_BISECT_RIGHT(a, x, lo, hi, key)


def insort_left(a, x, lo=0, hi=None, *, key=None):
    return _MOLT_BISECT_INSORT_LEFT(a, x, lo, hi, key)


def insort_right(a, x, lo=0, hi=None, *, key=None):
    return _MOLT_BISECT_INSORT_RIGHT(a, x, lo, hi, key)


globals().pop("_require_intrinsic", None)
