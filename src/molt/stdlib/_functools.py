"""Intrinsic-backed compatibility surface for CPython's `_functools`."""

from _intrinsics import require_intrinsic as _require_intrinsic

import sys as _sys
import functools as _functools_mod

_MOLT_PARTIAL = _require_intrinsic("molt_functools_partial")

cmp_to_key = _functools_mod.cmp_to_key
reduce = _functools_mod.reduce
partial = type(_MOLT_PARTIAL(lambda: None, (), {}))
del _MOLT_PARTIAL

__all__ = ["cmp_to_key", "partial", "reduce"]

if _sys.version_info >= (3, 14):
    _MOLT_KWD_MARK = _require_intrinsic("molt_functools_kwd_mark")
    Placeholder = _MOLT_KWD_MARK()
    del _MOLT_KWD_MARK
    __all__.insert(0, "Placeholder")
