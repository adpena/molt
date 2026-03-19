"""Array bisection algorithm.

Parity note: mirrors CPython's `bisect.py` public surface by exporting
`bisect`/`insort` aliases over `_bisect` core callables.
"""

from __future__ import annotations

import _bisect
from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_bisect_left")


def bisect_left(a, x, lo=0, hi=None, *, key=None):
    return _bisect.bisect_left(a, x, lo, hi, key)


def bisect_right(a, x, lo=0, hi=None, *, key=None):
    return _bisect.bisect_right(a, x, lo, hi, key)


def insort_left(a, x, lo=0, hi=None, *, key=None):
    _bisect.insort_left(a, x, lo, hi, key)


def insort_right(a, x, lo=0, hi=None, *, key=None):
    _bisect.insort_right(a, x, lo, hi, key)


bisect = bisect_right
insort = insort_right

globals().pop("_require_intrinsic", None)
