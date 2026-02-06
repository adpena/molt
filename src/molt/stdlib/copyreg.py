"""Pickle registry helpers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from collections.abc import Callable

_require_intrinsic("molt_stdlib_probe", globals())


__all__ = [
    "dispatch_table",
    "pickle",
    "constructor",
    "add_extension",
    "remove_extension",
    "clear_extension_cache",
]

dispatch_table: dict[type, Callable[[object], object]] = {}
_extension_registry: dict[tuple[str, str], int] = {}
_inverted_registry: dict[int, tuple[str, str]] = {}
_extension_cache: dict[int, object] = {}
_constructor_registry: set[Callable[..., object]] = set()


def pickle(
    cls: type,
    reducer: Callable[[object], object] | None,
    constructor_func: Callable[..., object] | None = None,
) -> None:
    if not isinstance(cls, type):
        raise TypeError("pickle() argument 1 must be a type")
    if reducer is None:
        dispatch_table.pop(cls, None)
    elif callable(reducer):
        dispatch_table[cls] = reducer
    else:
        raise TypeError("pickle() argument 2 must be callable or None")
    if constructor_func is not None:
        constructor(constructor_func)


def constructor(func: Callable[..., object]) -> Callable[..., object]:
    if not callable(func):
        raise TypeError("constructor() argument must be callable")
    _constructor_registry.add(func)
    return func


def _validate_extension_args(
    module: object, name: object, code: object
) -> tuple[str, str, int]:
    if not isinstance(module, str) or not module:
        raise ValueError("extension module name must be a non-empty string")
    if not isinstance(name, str) or not name:
        raise ValueError("extension name must be a non-empty string")
    if not isinstance(code, int):
        raise TypeError("extension code must be an int")
    if code <= 0:
        raise ValueError("extension code must be positive")
    return module, name, code


def add_extension(module: str, name: str, code: int) -> None:
    module, name, code = _validate_extension_args(module, name, code)
    key = (module, name)
    existing = _extension_registry.get(key)
    if existing is not None and existing != code:
        raise ValueError("extension already registered with a different code")
    existing_key = _inverted_registry.get(code)
    if existing_key is not None and existing_key != key:
        raise ValueError("extension code already in use")
    _extension_registry[key] = code
    _inverted_registry[code] = key


def remove_extension(module: str, name: str, code: int) -> None:
    module, name, code = _validate_extension_args(module, name, code)
    key = (module, name)
    existing = _extension_registry.get(key)
    if existing is None:
        raise ValueError("extension not registered")
    if existing != code:
        raise ValueError("extension code mismatch")
    _extension_registry.pop(key, None)
    _inverted_registry.pop(code, None)
    _extension_cache.pop(code, None)


def clear_extension_cache() -> None:
    _extension_cache.clear()
