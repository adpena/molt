"""Public API surface shim for ``asyncio.coroutines``."""

from __future__ import annotations

import collections.abc
import inspect
import os
import sys
import types


from typing import Any


_is_coroutine = object()

def _iscoroutinefunction(func: Any) -> bool:
    return inspect.iscoroutinefunction(func) or getattr(func, "_is_coroutine", None) is _is_coroutine


def iscoroutinefunction(func: Any) -> bool:
    if sys.version_info >= (3, 14):
        import warnings

        warnings._deprecated(
            "asyncio.iscoroutinefunction",
            f"{warnings._DEPRECATED_MSG}; use inspect.iscoroutinefunction() instead",
            remove=(3, 16),
        )
    return _iscoroutinefunction(func)


_iscoroutine_typecache: set[type] = set()

def iscoroutine(obj: Any) -> bool:
    # CPython's asyncio.coroutines.iscoroutine: a native coroutine or any
    # collections.abc.Coroutine (an async generator's asend/athrow awaitable
    # among them), with a positive type cache of at most 100 types.
    if type(obj) in _iscoroutine_typecache:
        return True
    if inspect.iscoroutine(obj) or isinstance(obj, collections.abc.Coroutine):
        if len(_iscoroutine_typecache) < 100:
            _iscoroutine_typecache.add(type(obj))
        return True
    return False

__all__ = ["iscoroutinefunction", "iscoroutine"]
