"""Intrinsic-backed helpers for `importlib.metadata` text handling."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from ._functools import method_cache
import re as re

_require_intrinsic("molt_stdlib_probe", globals())


class FoldedCase(str):
    @staticmethod
    def _coerce(other):
        if isinstance(other, str):
            return other
        return NotImplemented

    @method_cache
    def lower(self):
        return super().lower()

    def __hash__(self):
        return hash(self.lower())

    def __eq__(self, other):
        other_text = self._coerce(other)
        if other_text is NotImplemented:
            return NotImplemented
        return self.lower() == str(other_text).lower()

    def __lt__(self, other):
        other_text = self._coerce(other)
        if other_text is NotImplemented:
            return NotImplemented
        return self.lower() < str(other_text).lower()
