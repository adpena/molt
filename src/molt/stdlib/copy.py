"""Shallow/deep copy helpers for Molt."""

from __future__ import annotations

from typing import Any

__all__ = ["copy", "deepcopy"]

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): add
# copy dispatch tables, __reduce__ fallbacks, and broader slots coverage.


def _copy_molt_fields(
    obj: Any, result: Any, memo: dict[int, Any] | None = None
) -> bool:
    offsets = getattr(obj.__class__, "__molt_field_offsets__", None)
    if not isinstance(offsets, dict):
        return True
    for name in offsets:
        try:
            value = getattr(obj, name)
        except AttributeError:
            continue
        if memo is not None:
            value = deepcopy(value, memo)
        setattr(result, name, value)
    return True


def copy(obj: Any) -> Any:
    copier = getattr(obj, "__copy__", None)
    if callable(copier):
        return copier()
    if isinstance(obj, slice):
        return obj
    if isinstance(obj, list):
        return list(obj)
    if isinstance(obj, dict):
        return dict(obj)
    if isinstance(obj, tuple):
        return tuple(obj)
    if isinstance(obj, set):
        return set(obj)
    if hasattr(obj, "__dict__"):
        cls = obj.__class__
        try:
            result = cls.__new__(cls)
        except Exception:
            return obj
        try:
            _copy_molt_fields(obj, result, None)
            result.__dict__.update(obj.__dict__)
        except Exception:
            return obj
        return result
    return obj


def deepcopy(obj: Any, memo: dict[int, Any] | None = None) -> Any:
    if memo is None:
        memo = {}
    obj_id = id(obj)
    if obj_id in memo:
        return memo[obj_id]
    copier = getattr(obj, "__deepcopy__", None)
    if callable(copier):
        result = copier(memo)
        memo[obj_id] = result
        return result
    if isinstance(obj, slice):
        result = slice(obj.start, obj.stop, obj.step)
        memo[obj_id] = result
        return result
    if isinstance(obj, list):
        result: list[Any] = []
        memo[obj_id] = result
        for item in obj:
            result.append(deepcopy(item, memo))
        return result
    if isinstance(obj, dict):
        result: dict[Any, Any] = {}
        memo[obj_id] = result
        for key, value in obj.items():
            result[deepcopy(key, memo)] = deepcopy(value, memo)
        return result
    if isinstance(obj, tuple):
        items: list[Any] = []
        for item in obj:
            items.append(deepcopy(item, memo))
        result = tuple(items)
        memo[obj_id] = result
        return result
    if isinstance(obj, set):
        result: set[Any] = set()
        for item in obj:
            result.add(deepcopy(item, memo))
        memo[obj_id] = result
        return result
    if hasattr(obj, "__dict__"):
        cls = obj.__class__
        try:
            result = cls.__new__(cls)
        except Exception:
            memo[obj_id] = obj
            return obj
        memo[obj_id] = result
        try:
            _copy_molt_fields(obj, result, memo)
            for key, value in obj.__dict__.items():
                result.__dict__[key] = deepcopy(value, memo)
        except Exception:
            return result
        return result
    memo[obj_id] = obj
    return obj
