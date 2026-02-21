"""Intrinsic-backed helpers for `importlib.metadata` functional adapters."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import functools as functools
import types as types

_require_intrinsic("molt_stdlib_probe", globals())


def method_cache(method):
    cached = functools.lru_cache()(method)

    def wrapper(self, *args, **kwargs):
        return cached(self, *args, **kwargs)

    wrapper.cache_clear = cached.cache_clear
    return functools.update_wrapper(wrapper, method)


def pass_none(func):
    def wrapper(param, *args, **kwargs):
        if param is None:
            return None
        return func(param, *args, **kwargs)

    return functools.update_wrapper(wrapper, func)
