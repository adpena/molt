"""Intrinsic-backed compatibility surface for CPython's `_random`."""

from _intrinsics import require_intrinsic as _require_intrinsic

from random import Random

_require_intrinsic("molt_random_new")

__all__ = ["Random"]
