"""Shallow/deep copy helpers for Molt."""

from __future__ import annotations

from typing import Any

__all__ = ["copy", "deepcopy"]

# TODO(stdlib-compat, owner:stdlib, milestone:SL3): add copy dispatch tables + slots support.


def copy(obj: Any) -> Any:
    copier = getattr(obj, "__copy__", None)
    if callable(copier):
        return copier()
    if isinstance(obj, list):
        return list(obj)
    if isinstance(obj, dict):
        return dict(obj)
    if isinstance(obj, tuple):
        return tuple(obj)
    if isinstance(obj, set):
        return set(obj)
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
    memo[obj_id] = obj
    return obj
