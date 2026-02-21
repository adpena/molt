"""Intrinsic-backed helpers for `importlib.metadata` collection utilities."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import collections as collections

_require_intrinsic("molt_stdlib_probe", globals())


class FreezableDefaultDict(collections.defaultdict):
    def __init__(self, *args, **kwargs) -> None:
        super().__init__(*args, **kwargs)
        self._frozen = False

    def freeze(self) -> "FreezableDefaultDict":
        self._frozen = True
        return self

    def __missing__(self, key):
        if self._frozen:
            raise KeyError(key)
        return super().__missing__(key)


Pair = collections.namedtuple("Pair", "name value")
