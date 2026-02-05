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

_molt_heapq_heapify: Callable[[list[Any]], None] | None = _require_intrinsic(
    "molt_heapq_heapify"
)
_molt_heapq_heappush: Callable[[list[Any], Any], None] | None = _require_intrinsic(
    "molt_heapq_heappush"
)
_molt_heapq_heappop: Callable[[list[Any]], Any] | None = _require_intrinsic(
    "molt_heapq_heappop"
)
_molt_heapq_heapreplace: Callable[[list[Any], Any], Any] | None = _require_intrinsic(
    "molt_heapq_heapreplace"
)
_molt_heapq_heappushpop: Callable[[list[Any], Any], Any] | None = _require_intrinsic(
    "molt_heapq_heappushpop"
)


def _siftdown(heap: list[T], startpos: int, pos: int) -> None:
    newitem = heap[pos]
    while pos > startpos:
        parentpos = (pos - 1) // 2
        parent = heap[parentpos]
        if newitem < parent:
            heap[pos] = parent
            pos = parentpos
            continue
        break
    heap[pos] = newitem


def _siftup(heap: list[T], pos: int) -> None:
    endpos = len(heap)
    startpos = pos
    newitem = heap[pos]
    childpos = 2 * pos + 1
    while childpos < endpos:
        rightpos = childpos + 1
        if rightpos < endpos and not heap[childpos] < heap[rightpos]:
            childpos = rightpos
        heap[pos] = heap[childpos]
        pos = childpos
        childpos = 2 * pos + 1
    heap[pos] = newitem
    _siftdown(heap, startpos, pos)


def _siftdown_max(heap: list[T], startpos: int, pos: int) -> None:
    newitem = heap[pos]
    while pos > startpos:
        parentpos = (pos - 1) // 2
        parent = heap[parentpos]
        if parent < newitem:
            heap[pos] = parent
            pos = parentpos
            continue
        break
    heap[pos] = newitem


def _siftup_max(heap: list[T], pos: int) -> None:
    endpos = len(heap)
    startpos = pos
    newitem = heap[pos]
    childpos = 2 * pos + 1
    while childpos < endpos:
        rightpos = childpos + 1
        if rightpos < endpos and not heap[rightpos] < heap[childpos]:
            childpos = rightpos
        heap[pos] = heap[childpos]
        pos = childpos
        childpos = 2 * pos + 1
    heap[pos] = newitem
    _siftdown_max(heap, startpos, pos)


def _heapify_max(x: list[T]) -> None:
    for idx in range(len(x) // 2 - 1, -1, -1):
        _siftup_max(x, idx)


def _heappop_max(heap: list[T]) -> T:
    if not heap:
        raise IndexError("index out of range")
    lastelt = heap.pop()
    if not heap:
        return lastelt
    returnitem = heap[0]
    heap[0] = lastelt
    _siftup_max(heap, 0)
    return returnitem


class _ReverseKey:
    __slots__ = ("key",)

    def __init__(self, key: Any) -> None:
        self.key = key

    def __lt__(self, other: "_ReverseKey") -> bool:
        return other.key < self.key


def heapify(x: list[T]) -> None:
    if type(x) is list:
        _molt_heapq_heapify(x)  # type: ignore[unresolved-reference]
        return
    for idx in range(len(x) // 2 - 1, -1, -1):
        _siftup(x, idx)


def heappush(heap: list[T], item: T) -> None:
    if type(heap) is list:
        _molt_heapq_heappush(heap, item)  # type: ignore[unresolved-reference]
        return
    heap.append(item)
    _siftdown(heap, 0, len(heap) - 1)


def heappop(heap: list[T]) -> T:
    if type(heap) is list:
        return _molt_heapq_heappop(heap)  # type: ignore[unresolved-reference]
    if not heap:
        raise IndexError("index out of range")
    lastelt = heap.pop()
    if not heap:
        return lastelt
    returnitem = heap[0]
    heap[0] = lastelt
    _siftup(heap, 0)
    return returnitem


def heapreplace(heap: list[T], item: T) -> T:
    if type(heap) is list:
        return _molt_heapq_heapreplace(heap, item)  # type: ignore[unresolved-reference]
    if not heap:
        raise IndexError("index out of range")
    returnitem = heap[0]
    heap[0] = item
    _siftup(heap, 0)
    return returnitem


def heappushpop(heap: list[T], item: T) -> T:
    if type(heap) is list:
        return _molt_heapq_heappushpop(heap, item)  # type: ignore[unresolved-reference]
    if heap and heap[0] < item:
        returnitem = heap[0]
        heap[0] = item
        _siftup(heap, 0)
        return returnitem
    return item


def nsmallest(
    n: Any, iterable: Iterable[T], key: Callable[[T], Any] | None = None
) -> list[T]:
    data = list(iterable)
    if n <= 0:
        return []
    if n >= len(data):
        return sorted(data, key=key)
    return sorted(data, key=key)[:n]


def nlargest(
    n: Any, iterable: Iterable[T], key: Callable[[T], Any] | None = None
) -> list[T]:
    data = list(iterable)
    if n >= len(data):
        return sorted(data, key=key, reverse=True)
    if n <= 0:
        return []
    if key is not None:
        return sorted(data, key=key, reverse=True)[:n]
    heap = data[:n]
    heapify(heap)
    for item in data[n:]:
        if heap[0] < item:
            heap[0] = item
            _siftup(heap, 0)
    heap.sort(reverse=True)
    return heap


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
    data: list[T] = []
    for iterable in iterables:
        for item in iterable:
            data.append(item)
    data.sort(key=key, reverse=reverse)
    return iter(data)
