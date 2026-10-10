"""Intrinsic-backed compatibility surface for CPython's `_warnings`."""


from warnings import _filters as filters
from warnings import warn, warn_explicit


__all__ = [
    "filters",
    "warn",
    "warn_explicit",
]
