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

_MOLT_ADD = _require_intrinsic("molt_operator_add", globals())
_MOLT_MUL = _require_intrinsic("molt_operator_mul", globals())
_MOLT_EQ = _require_intrinsic("molt_operator_eq", globals())
_MOLT_INDEX = _require_intrinsic("molt_operator_index", globals())
_MOLT_ITEMGETTER = _require_intrinsic("molt_operator_itemgetter", globals())
_MOLT_ATTRGETTER = _require_intrinsic("molt_operator_attrgetter", globals())
_MOLT_METHODCALLER = _require_intrinsic("molt_operator_methodcaller", globals())


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
