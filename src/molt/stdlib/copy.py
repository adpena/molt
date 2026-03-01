"""Shallow/deep copy helpers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from typing import Any, Callable

_molt_copy_copy = _require_intrinsic("molt_copy_copy", globals())
_molt_copy_deepcopy = _require_intrinsic("molt_copy_deepcopy", globals())
_molt_copy_memo_new = _require_intrinsic("molt_copy_memo_new", globals())
_molt_copy_memo_drop = _require_intrinsic("molt_copy_memo_drop", globals())
_molt_copy_error = _require_intrinsic("molt_copy_error", globals())

__all__ = ["copy", "deepcopy", "Error", "dispatch_table"]


class Error(Exception):
    def __init__(self, msg: str = "") -> None:
        super().__init__(msg)
        # Notify the Rust runtime so it can record the error in the copy
        # diagnostics lane (e.g., for structured error reporting / tests).
        try:
            _molt_copy_error(str(msg))
        except Exception:
            pass


dispatch_table: dict[type, Callable[[Any], Any]] = {}


def copy(obj: Any) -> Any:
    return _molt_copy_copy(obj)


def deepcopy(obj: Any, memo: dict[int, Any] | None = None) -> Any:
    return _molt_copy_deepcopy(obj, None)
