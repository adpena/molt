"""Intrinsic-backed helpers for `importlib.metadata` functional adapters."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import functools as functools
import types as types

_WRAPPER_ASSIGNMENTS = functools.WRAPPER_ASSIGNMENTS
_WRAPPER_UPDATES = functools.WRAPPER_UPDATES

_MOLT_LRU_CACHE = _require_intrinsic("molt_functools_lru_cache")
_MOLT_UPDATE_WRAPPER = _require_intrinsic("molt_functools_update_wrapper")


def method_cache(method):
    cached = _MOLT_LRU_CACHE(128, False)(method)

    def wrapper(self, *args, **kwargs):
        return cached(self, *args, **kwargs)

    wrapper.cache_clear = cached.cache_clear
    return _MOLT_UPDATE_WRAPPER(wrapper, method, _WRAPPER_ASSIGNMENTS, _WRAPPER_UPDATES)


def pass_none(func):
    def wrapper(param, *args, **kwargs):
        if param is None:
            return None
        return func(param, *args, **kwargs)

    return _MOLT_UPDATE_WRAPPER(wrapper, func, _WRAPPER_ASSIGNMENTS, _WRAPPER_UPDATES)
