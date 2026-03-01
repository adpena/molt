"""Shallow/deep copy helpers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from typing import Any, Callable

_molt_copy_copy = _require_intrinsic("molt_copy_copy", globals())
_molt_copy_deepcopy = _require_intrinsic("molt_copy_deepcopy", globals())
_molt_copy_memo_new = _require_intrinsic("molt_copy_memo_new", globals())
_molt_copy_memo_drop = _require_intrinsic("molt_copy_memo_drop", globals())

__all__ = ["copy", "deepcopy", "Error", "dispatch_table"]


class Error(Exception):
    pass


dispatch_table: dict[type, Callable[[Any], Any]] = {}


def copy(obj: Any) -> Any:
    return _molt_copy_copy(obj)


def deepcopy(obj: Any, memo: dict[int, Any] | None = None) -> Any:
    return _molt_copy_deepcopy(obj, None)
