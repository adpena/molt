"""Intrinsic-backed compatibility surface for CPython's `_py_abc`."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from _abc import get_cache_token as _abc_get_cache_token
from _weakrefset import WeakSet
from abc import ABCMeta

_require_intrinsic("molt_capabilities_has", globals())


def get_cache_token():
    return _abc_get_cache_token()


__all__ = ["ABCMeta", "WeakSet", "get_cache_token"]
