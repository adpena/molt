"""Intrinsic-backed pickle registry helpers."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from collections.abc import Callable


__all__ = [
    "dispatch_table",
    "pickle",
    "constructor",
    "add_extension",
    "remove_extension",
    "clear_extension_cache",
]

_MOLT_COPYREG_BOOTSTRAP = _require_intrinsic("molt_copyreg_bootstrap", globals())
_MOLT_COPYREG_PICKLE = _require_intrinsic("molt_copyreg_pickle", globals())
_MOLT_COPYREG_CONSTRUCTOR = _require_intrinsic("molt_copyreg_constructor", globals())
_MOLT_COPYREG_ADD_EXTENSION = _require_intrinsic(
    "molt_copyreg_add_extension", globals()
)
_MOLT_COPYREG_REMOVE_EXTENSION = _require_intrinsic(
    "molt_copyreg_remove_extension", globals()
)
_MOLT_COPYREG_CLEAR_EXTENSION_CACHE = _require_intrinsic(
    "molt_copyreg_clear_extension_cache", globals()
)

_state = _MOLT_COPYREG_BOOTSTRAP()
if not isinstance(_state, (tuple, list)) or len(_state) != 5:
    raise RuntimeError("copyreg bootstrap intrinsic returned invalid state")

dispatch_table = _state[0]
_extension_registry = _state[1]
_inverted_registry = _state[2]
_extension_cache = _state[3]
_constructor_registry = _state[4]
if not isinstance(dispatch_table, dict):
    raise RuntimeError("copyreg bootstrap intrinsic returned invalid dispatch table")
if not isinstance(_extension_registry, dict):
    raise RuntimeError(
        "copyreg bootstrap intrinsic returned invalid extension registry"
    )
if not isinstance(_inverted_registry, dict):
    raise RuntimeError("copyreg bootstrap intrinsic returned invalid inverted registry")
if not isinstance(_extension_cache, dict):
    raise RuntimeError("copyreg bootstrap intrinsic returned invalid extension cache")
if not isinstance(_constructor_registry, set):
    raise RuntimeError(
        "copyreg bootstrap intrinsic returned invalid constructor registry"
    )


def pickle(
    cls: type,
    reducer: Callable[[object], object],
    constructor_func: Callable[..., object] | None = None,
) -> None:
    _MOLT_COPYREG_PICKLE(cls, reducer, constructor_func)
    return None


def constructor(func: Callable[..., object]) -> None:
    _MOLT_COPYREG_CONSTRUCTOR(func)
    return None


def add_extension(module: str, name: str, code: int) -> None:
    _MOLT_COPYREG_ADD_EXTENSION(module, name, code)
    return None


def remove_extension(module: str, name: str, code: int) -> None:
    _MOLT_COPYREG_REMOVE_EXTENSION(module, name, code)
    return None


def clear_extension_cache() -> None:
    _MOLT_COPYREG_CLEAR_EXTENSION_CACHE()
    return None
