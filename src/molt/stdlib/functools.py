"""Deterministic functools shim for Molt.

Functools helpers are backed by runtime intrinsics; missing intrinsics are a hard error.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


from typing import Any, Callable, Iterable, TYPE_CHECKING


if TYPE_CHECKING:
    pass

__all__ = [
    "cmp_to_key",
    "lru_cache",
    "partial",
    "reduce",
    "total_ordering",
    "update_wrapper",
    "wraps",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P2, status:partial): finish
# functools surface (namedtuple cache_info, singledispatch).

WRAPPER_ASSIGNMENTS = (
    "__module__",
    "__name__",
    "__qualname__",
    "__doc__",
    "__annotations__",
)
WRAPPER_UPDATES = ("__dict__",)


def _as_callable(name: str):
    return _require_intrinsic(name, globals())


_MOLT_KWD_MARK = _as_callable("molt_functools_kwd_mark")
_MOLT_UPDATE_WRAPPER = _as_callable("molt_functools_update_wrapper")
_MOLT_WRAPS = _as_callable("molt_functools_wraps")
_MOLT_CMP_TO_KEY = _as_callable("molt_functools_cmp_to_key")
_MOLT_TOTAL_ORDERING = _as_callable("molt_functools_total_ordering")
_MOLT_PARTIAL = _as_callable("molt_functools_partial")
_MOLT_REDUCE = _as_callable("molt_functools_reduce")
_MOLT_LRU_CACHE = _as_callable("molt_functools_lru_cache")

_MISSING = _MOLT_KWD_MARK()


def update_wrapper(
    wrapper: Callable[..., Any],
    wrapped: Callable[..., Any],
    assigned: Iterable[str] = WRAPPER_ASSIGNMENTS,
    updated: Iterable[str] = WRAPPER_UPDATES,
) -> Callable[..., Any]:
    return _MOLT_UPDATE_WRAPPER(wrapper, wrapped, assigned, updated)


def wraps(
    wrapped: Callable[..., Any],
    assigned: Iterable[str] = WRAPPER_ASSIGNMENTS,
    updated: Iterable[str] = WRAPPER_UPDATES,
) -> Callable[[Callable[..., Any]], Callable[..., Any]]:
    return _MOLT_WRAPS(wrapped, assigned, updated)


def cmp_to_key(mycmp: Callable[[Any, Any], Any]) -> Any:
    return _MOLT_CMP_TO_KEY(mycmp)


def total_ordering(cls: type[Any]) -> type[Any]:
    return _MOLT_TOTAL_ORDERING(cls)


def partial(func: Callable[..., Any], /, *args: Any, **kwargs: Any) -> Any:
    return _MOLT_PARTIAL(func, args, kwargs)


def reduce(
    function: Callable[[Any, Any], Any],
    iterable: Iterable[Any],
    initializer: Any = _MISSING,
) -> Any:
    return _MOLT_REDUCE(function, iterable, initializer)


def lru_cache(maxsize: Any = 128, typed: bool = False):
    return _MOLT_LRU_CACHE(maxsize, typed)
