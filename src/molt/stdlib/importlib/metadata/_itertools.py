"""Intrinsic-backed helpers for `importlib.metadata` iterable utilities."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from itertools import filterfalse as filterfalse

_MOLT_FILTERFALSE = _require_intrinsic("molt_itertools_filterfalse")


def always_iterable(obj, base_type=(str, bytes)):
    if obj is None:
        return iter(())
    if base_type is not None and isinstance(obj, base_type):
        return iter((obj,))
    try:
        return iter(obj)
    except TypeError:
        return iter((obj,))


def unique_everseen(iterable, key=None):
    if key is None:
        seen = set()
        seen_add = seen.add
        for element in _MOLT_FILTERFALSE(seen.__contains__, iterable):
            seen_add(element)
            yield element
        return
    seen_keys = set()
    seen_keys_add = seen_keys.add
    for element in iterable:
        marker = key(element)
        if marker in seen_keys:
            continue
        seen_keys_add(marker)
        yield element

globals().pop("_require_intrinsic", None)
