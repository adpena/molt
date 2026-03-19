"""Heap queue algorithm helpers for Molt."""

from __future__ import annotations

from typing import Any, Callable, Iterable, Protocol, TypeVar

from _intrinsics import require_intrinsic as _require_intrinsic


__all__ = [
    "heapify",
    "heappush",
    "heappop",
    "heapreplace",
    "heappushpop",
    "merge",
    "nlargest",
    "nsmallest",
]


class _SupportsLessThan(Protocol):
    def __lt__(self, other: Any, /) -> bool: ...


T = TypeVar("T", bound=_SupportsLessThan)

_molt_heapq_heapify = _require_intrinsic("molt_heapq_heapify")
_molt_heapq_heappush = _require_intrinsic("molt_heapq_heappush")
_molt_heapq_heappop = _require_intrinsic("molt_heapq_heappop")
_molt_heapq_heapreplace = _require_intrinsic("molt_heapq_heapreplace")
_molt_heapq_heappushpop = _require_intrinsic("molt_heapq_heappushpop")
_molt_heapq_heapify_max = _require_intrinsic("molt_heapq_heapify_max")
_molt_heapq_heappop_max = _require_intrinsic("molt_heapq_heappop_max")
_molt_heapq_nsmallest = _require_intrinsic("molt_heapq_nsmallest")
_molt_heapq_nlargest = _require_intrinsic("molt_heapq_nlargest")
_molt_heapq_merge = _require_intrinsic("molt_heapq_merge")


def heapify(x: list[T]) -> None:
    _molt_heapq_heapify(x)


def heappush(heap: list[T], item: T) -> None:
    _molt_heapq_heappush(heap, item)


def heappop(heap: list[T]) -> T:
    return _molt_heapq_heappop(heap)


def heapreplace(heap: list[T], item: T) -> T:
    return _molt_heapq_heapreplace(heap, item)


def heappushpop(heap: list[T], item: T) -> T:
    return _molt_heapq_heappushpop(heap, item)


def nsmallest(
    n: Any, iterable: Iterable[T], key: Callable[[T], Any] | None = None
) -> list[T]:
    return _molt_heapq_nsmallest(n, iterable, key)


def nlargest(
    n: Any, iterable: Iterable[T], key: Callable[[T], Any] | None = None
) -> list[T]:
    return _molt_heapq_nlargest(n, iterable, key)


def merge(*iterables: Iterable[T], **kwargs):
    key: Callable[[T], Any] | None = None
    reverse = False
    if kwargs:
        if "key" in kwargs:
            key = kwargs.pop("key")
        if "reverse" in kwargs:
            reverse = kwargs.pop("reverse")
        if kwargs:
            raise TypeError("merge() got unexpected keyword arguments")
    return iter(_molt_heapq_merge(list(iterables), key, reverse))

globals().pop("_require_intrinsic", None)
