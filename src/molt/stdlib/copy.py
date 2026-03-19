"""Shallow/deep copy helpers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from typing import Any, Callable

_molt_copy_copy = _require_intrinsic("molt_copy_copy")
_molt_copy_deepcopy = _require_intrinsic("molt_copy_deepcopy")
_molt_copy_memo_new = _require_intrinsic("molt_copy_memo_new")
_molt_copy_memo_drop = _require_intrinsic("molt_copy_memo_drop")
_molt_copy_error = _require_intrinsic("molt_copy_error")
_molt_copy_replace = _require_intrinsic("molt_copy_replace")

__all__ = ["copy", "deepcopy", "replace", "Error", "dispatch_table"]


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
    """Create a shallow copy of obj."""
    return _molt_copy_copy(obj)


def deepcopy(obj: Any, memo: dict[int, Any] | None = None) -> Any:
    """Create a deep copy of obj, using memo to track already-copied objects."""
    if memo is not None:
        # Use a Rust-side memo handle for the operation.
        handle = _molt_copy_memo_new()
        try:
            result = _molt_copy_deepcopy(obj, handle)
        finally:
            _molt_copy_memo_drop(handle)
        return result
    return _molt_copy_deepcopy(obj, None)


def replace(obj: Any, /, **changes: Any) -> Any:
    """Return a modified copy of obj with the given attribute changes applied."""
    cls = type(obj)
    # Check for __replace__ protocol (PEP 618 / 3.13+)
    replacer = getattr(cls, "__replace__", None)
    if replacer is not None:
        return replacer(obj, **changes)
    # For namedtuple-like objects with _replace
    _replace = getattr(obj, "_replace", None)
    if _replace is not None:
        return _replace(**changes)
    # Generic fallback: shallow copy then apply changes
    new = _molt_copy_copy(obj)
    for key, value in changes.items():
        setattr(new, key, value)
    return new

globals().pop("_require_intrinsic", None)
