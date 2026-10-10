"""Array bisection algorithm.

Parity note: mirrors CPython's `bisect.py` public surface by exporting
`bisect`/`insort` aliases over `_bisect` core callables.
"""

from __future__ import annotations

import _bisect


def bisect_left(a, x, lo=0, hi=None, *, key=None):
    return _bisect.bisect_left(a, x, lo, hi, key=key)


def bisect_right(a, x, lo=0, hi=None, *, key=None):
    return _bisect.bisect_right(a, x, lo, hi, key=key)


def insort_left(a, x, lo=0, hi=None, *, key=None):
    _bisect.insort_left(a, x, lo, hi, key=key)


def insort_right(a, x, lo=0, hi=None, *, key=None):
    _bisect.insort_right(a, x, lo, hi, key=key)


bisect = bisect_right
insort = insort_right
