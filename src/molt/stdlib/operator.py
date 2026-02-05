"""Operator helpers for Molt.

Operator helpers are backed by runtime intrinsics; missing intrinsics are a hard error.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


try:
    from typing import TYPE_CHECKING
except Exception:
    TYPE_CHECKING = False

if TYPE_CHECKING:
    from typing import Any, Callable
else:
    Any = object()
    Callable = object()

__all__ = [
    "add",
    "attrgetter",
    "eq",
    "index",
    "itemgetter",
    "methodcaller",
    "mul",
]


def _as_callable(name: str):
    return _require_intrinsic(name, globals())


_MOLT_ADD = _as_callable("molt_operator_add")
_MOLT_MUL = _as_callable("molt_operator_mul")
_MOLT_EQ = _as_callable("molt_operator_eq")
_MOLT_INDEX = _as_callable("molt_operator_index")
_MOLT_ITEMGETTER = _as_callable("molt_operator_itemgetter")
_MOLT_ATTRGETTER = _as_callable("molt_operator_attrgetter")
_MOLT_METHODCALLER = _as_callable("molt_operator_methodcaller")


def add(a: Any, b: Any) -> Any:
    return _MOLT_ADD(a, b)


def mul(a: Any, b: Any) -> Any:
    return _MOLT_MUL(a, b)


def eq(a: Any, b: Any) -> bool:
    return _MOLT_EQ(a, b)


def index(a: Any) -> int:
    return _MOLT_INDEX(a)


def itemgetter(*items: Any) -> Callable[[Any], Any]:
    if not items:
        raise TypeError("itemgetter expected at least 1 argument")
    return _MOLT_ITEMGETTER(items)


def attrgetter(*attrs: str) -> Callable[[Any], Any]:
    if not attrs:
        raise TypeError("attrgetter expected at least 1 argument")
    return _MOLT_ATTRGETTER(attrs)


def methodcaller(name: str, *args: Any, **kwargs: Any) -> Callable[[Any], Any]:
    return _MOLT_METHODCALLER(name, args, kwargs)
