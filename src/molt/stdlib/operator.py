"""Operator helpers for Molt."""

from __future__ import annotations

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
    "itemgetter",
    "methodcaller",
    "mul",
]


def add(a: Any, b: Any) -> Any:
    return a + b


def mul(a: Any, b: Any) -> Any:
    return a * b


def eq(a: Any, b: Any) -> bool:
    return a == b


class _ItemGetter:
    def __init__(self, items: tuple[Any, ...]) -> None:
        self._items = items

    def __call__(self, obj: Any) -> Any:
        count = len(self._items)
        if count == 1:
            return obj[self._items[0]]
        out: list[Any] = []
        idx = 0
        while idx < count:
            out.append(obj[self._items[idx]])
            idx += 1
        return tuple(out)


def itemgetter(*items: Any) -> _ItemGetter:
    if not items:
        raise TypeError("itemgetter expected at least 1 argument")
    return _ItemGetter(items)


class _AttrGetter:
    def __init__(self, attrs: tuple[str, ...]) -> None:
        self._attrs = attrs

    def __call__(self, obj: Any) -> Any:
        count = len(self._attrs)
        if count == 1:
            return _resolve_attr(obj, self._attrs[0])
        out: list[Any] = []
        idx = 0
        while idx < count:
            out.append(_resolve_attr(obj, self._attrs[idx]))
            idx += 1
        return tuple(out)


def _resolve_attr(obj: Any, name: str) -> Any:
    if "." in name:
        current = obj
        for part in name.split("."):
            current = getattr(current, part)
        return current
    return getattr(obj, name)


def attrgetter(*attrs: str) -> _AttrGetter:
    if not attrs:
        raise TypeError("attrgetter expected at least 1 argument")
    return _AttrGetter(attrs)


class _MethodCaller:
    def __init__(
        self, name: str, args: tuple[Any, ...], kwargs: dict[str, Any]
    ) -> None:
        self._name = name
        out: list[Any] = []
        idx = 0
        count = len(args)
        while idx < count:
            out.append(args[idx])
            idx += 1
        self._args = out
        self._kwargs = kwargs

    def __call__(self, obj: Any) -> Any:
        method = getattr(obj, self._name)
        return method(*self._args, **self._kwargs)


def methodcaller(name: str, *args: Any, **kwargs: Any) -> Callable[[Any], Any]:
    return _MethodCaller(name, args, kwargs)
