"""Intrinsic-backed compatibility surface for CPython's `_warnings`."""

from _intrinsics import require_intrinsic as _require_intrinsic

from warnings import _filters as filters
from warnings import warn, warn_explicit

_MOLT_WARNINGS_WARN = _require_intrinsic("molt_warnings_warn")

__all__ = [
    "filters",
    "warn",
    "warn_explicit",
]

globals().pop("_require_intrinsic", None)
