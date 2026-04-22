"""Keyword helpers for Molt (Python 3.12+)."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_keyword_lists = _require_intrinsic("molt_keyword_lists")
_is_keyword_intrinsic = _require_intrinsic("molt_keyword_iskeyword")
_is_soft_keyword_intrinsic = _require_intrinsic("molt_keyword_issoftkeyword")

__all__ = ["kwlist", "softkwlist", "iskeyword", "issoftkeyword"]

kwlist, softkwlist = _keyword_lists()


def iskeyword(value) -> bool:
    return bool(_is_keyword_intrinsic(value))


def issoftkeyword(value) -> bool:
    return bool(_is_soft_keyword_intrinsic(value))


globals().pop("_require_intrinsic", None)
