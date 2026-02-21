"""Intrinsic-backed helpers for `importlib.metadata` iterable utilities."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from itertools import filterfalse as filterfalse

_require_intrinsic("molt_stdlib_probe", globals())


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
        for element in filterfalse(seen.__contains__, iterable):
            seen.add(element)
            yield element
        return
    seen_keys = set()
    for element in iterable:
        marker = key(element)
        if marker in seen_keys:
            continue
        seen_keys.add(marker)
        yield element
