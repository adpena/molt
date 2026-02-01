"""Iterator helpers for Molt."""

from __future__ import annotations

import operator
from typing import (
    Any,
    Callable,
    Generic,
    Iterable,
    Iterator,
    TYPE_CHECKING,
    TypeVar,
    cast,
)

__all__ = [
    "accumulate",
    "chain",
    "combinations",
    "count",
    "cycle",
    "groupby",
    "islice",
    "pairwise",
    "permutations",
    "product",
    "repeat",
    "tee",
]

T = TypeVar("T")
_SENTINEL = object()


class _ChainIter:
    def __init__(self, iterables: Iterable[Iterable[T]]) -> None:
        self._iterables = iter(iterables)
        self._current: Iterator[T] | None = None

    def __iter__(self):
        return self

    def __next__(self):
        while True:
            if self._current is None:
                next_iterable = next(self._iterables, _SENTINEL)
                if next_iterable is _SENTINEL:
                    raise StopIteration
                self._current = iter(cast(Iterable[T], next_iterable))
            item = next(self._current, _SENTINEL)
            if item is _SENTINEL:
                self._current = None
                continue
            return item


def chain(*iterables: Iterable[T]) -> Iterator[T]:
    return _ChainIter(iterables)


def _chain_from_iterable(iterables: Iterable[Iterable[T]]) -> Iterator[T]:
    return _ChainIter(iterables)


chain.from_iterable = _chain_from_iterable  # type: ignore[attr-defined]


class _IsliceIter:
    def __init__(
        self,
        iterable: Iterable[T],
        start: int,
        stop: int | None,
        step: int,
    ) -> None:
        self._iter = iter(iterable)
        self._stop = stop
        self._step = step
        self._idx = 0
        self._next_index = start

    def __iter__(self):
        return self

    def __next__(self):
        if self._stop is not None and self._next_index >= self._stop:
            raise StopIteration
        while True:
            if self._stop is not None and self._idx >= self._stop:
                raise StopIteration
            item = next(self._iter, _SENTINEL)
            if item is _SENTINEL:
                raise StopIteration
            if self._idx == self._next_index:
                self._idx += 1
                self._next_index += self._step
                return item
            self._idx += 1


def islice(
    iterable: Iterable[T],
    start: int,
    stop: int | None = None,
    step: int = 1,
) -> Iterator[T]:
    stop_only = stop is None
    if stop is None:
        stop = start
        start = 0
    if start < 0 or (stop is not None and stop < 0):
        if stop_only:
            raise ValueError(
                "Stop argument for islice() must be None or an integer: 0 <= x <= sys.maxsize."
            )
        raise ValueError(
            "Indices for islice() must be None or an integer: 0 <= x <= sys.maxsize."
        )
    if step <= 0:
        raise ValueError("Step for islice() must be a positive integer or None.")
    return _IsliceIter(iterable, start, stop, step)


class _RepeatIter:
    def __init__(self, obj: T, times: int | None) -> None:
        self._obj = obj
        self._times = times
        self._index = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self._times is None:
            return self._obj
        if self._index >= self._times:
            raise StopIteration
        self._index += 1
        return self._obj


def repeat(obj: T, times: int | None = None) -> Iterator[T]:
    return _RepeatIter(obj, times)


class _CountIter:
    def __init__(self, start: int, step: int) -> None:
        self._current = start
        self._step = step

    def __iter__(self):
        return self

    def __next__(self):
        value = self._current
        self._current += self._step
        return value


def count(start: int = 0, step: int = 1) -> Iterator[int]:
    return _CountIter(start, step)


class _CycleIter:
    def __init__(self, iterable: Iterable[T]) -> None:
        self._saved = list(iterable)
        self._index = 0

    def __iter__(self):
        return self

    def __next__(self):
        if not self._saved:
            raise StopIteration
        value = self._saved[self._index]
        self._index = (self._index + 1) % len(self._saved)
        return value


def cycle(iterable: Iterable[T]) -> Iterator[T]:
    return _CycleIter(iterable)


class _AccumulateIter:
    def __init__(
        self,
        iterable: Iterable[T],
        func: Callable[[Any, Any], Any],
        initial: Any,
    ) -> None:
        self._iter = iter(iterable)
        self._func = func
        self._initial = initial
        self._started = False
        self._total: Any = None

    def __iter__(self):
        return self

    def __next__(self):
        if not self._started:
            self._started = True
            if self._initial is not _SENTINEL:
                self._total = self._initial
                return self._total
            item = next(self._iter, _SENTINEL)
            if item is _SENTINEL:
                raise StopIteration
            self._total = item
            return self._total
        item = next(self._iter, _SENTINEL)
        if item is _SENTINEL:
            raise StopIteration
        self._total = self._func(self._total, item)
        return self._total


def accumulate(
    iterable: Iterable[T],
    func: Callable[[Any, Any], Any] = operator.add,
    initial: Any = _SENTINEL,
) -> Iterator[Any]:
    return _AccumulateIter(iterable, func, initial)


if TYPE_CHECKING:
    _PairwiseIterBase = Generic[T]
else:
    _PairwiseIterBase = object


class _PairwiseIter(_PairwiseIterBase):
    def __init__(self, iterable: Iterable[T]) -> None:
        self._iter = iter(iterable)
        self._started = False
        self._prev: T | None = None

    def __iter__(self):
        return self

    def __next__(self):
        if not self._started:
            first = next(self._iter, _SENTINEL)
            if first is _SENTINEL:
                raise StopIteration
            self._prev = cast(T, first)
            self._started = True
        item = next(self._iter, _SENTINEL)
        if item is _SENTINEL:
            raise StopIteration
        item_t = cast(T, item)
        pair = (self._prev, item_t)
        self._prev = item_t
        return pair


def pairwise(iterable: Iterable[T]) -> Iterator[tuple[T, T]]:
    return _PairwiseIter(iterable)


def product(*iterables: Iterable[T], repeat: int = 1) -> Iterator[tuple[T, ...]]:
    if repeat <= 0:
        return iter([()])
    pools: list[tuple[T, ...]] = []
    for iterable in iterables:
        pools.append(tuple(iterable))
    pools = pools * repeat
    if not pools:
        return iter([()])
    for pool in pools:
        if not pool:
            return iter([])
    indices = [0] * len(pools)
    result = [pool[0] for pool in pools]
    out: list[tuple[T, ...]] = [tuple(result)]
    while True:
        idx = len(pools) - 1
        advanced = False
        while idx >= 0:
            pool = pools[idx]
            if indices[idx] + 1 < len(pool):
                indices[idx] += 1
                result[idx] = pool[indices[idx]]
                for jdx in range(idx + 1, len(pools)):
                    indices[jdx] = 0
                    result[jdx] = pools[jdx][0]
                out.append(tuple(result))
                advanced = True
                break
            idx -= 1
        if not advanced:
            break
    return iter(out)


def permutations(
    iterable: Iterable[T], r: int | None = None
) -> Iterator[tuple[T, ...]]:
    pool = tuple(iterable)
    n = len(pool)
    if r is None:
        r = n
    if r < 0 or r > n:
        return iter([])
    indices = list(range(n))
    cycles = list(range(n, n - r, -1))
    out: list[tuple[T, ...]] = [tuple(pool[i] for i in indices[:r])]
    while n:
        idx = r - 1
        advanced = False
        while idx >= 0:
            cycles[idx] -= 1
            if cycles[idx] == 0:
                indices[idx:] = indices[idx + 1 :] + indices[idx : idx + 1]
                cycles[idx] = n - idx
            else:
                jdx = cycles[idx]
                indices[idx], indices[-jdx] = indices[-jdx], indices[idx]
                out.append(tuple(pool[i] for i in indices[:r]))
                advanced = True
                break
            idx -= 1
        if not advanced:
            break
    return iter(out)


def combinations(iterable: Iterable[T], r: int) -> Iterator[tuple[T, ...]]:
    pool = tuple(iterable)
    n = len(pool)
    if r < 0 or r > n:
        return iter([])
    indices = list(range(r))
    out: list[tuple[T, ...]] = [tuple(pool[i] for i in indices)]
    while True:
        idx = r - 1
        found = False
        while idx >= 0:
            if indices[idx] != idx + n - r:
                found = True
                break
            idx -= 1
        if not found:
            break
        indices[idx] += 1
        for jdx in range(idx + 1, r):
            indices[jdx] = indices[jdx - 1] + 1
        out.append(tuple(pool[i] for i in indices))
    return iter(out)


class _GroupBy:
    def __init__(self, iterable: Iterable[T], key: Callable[[T], Any] | None) -> None:
        self._it = iter(iterable)
        self._key = key or (lambda value: value)
        self._tgt_key = _SENTINEL
        self._curr_key = _SENTINEL
        self._curr_val = _SENTINEL
        self._done = False

    def __iter__(self):
        return self

    def __next__(self):
        if self._done:
            raise StopIteration
        if self._curr_key is _SENTINEL:
            self._advance()
            if self._done:
                raise StopIteration
        while self._tgt_key is not _SENTINEL and self._curr_key == self._tgt_key:
            self._advance()
            if self._done:
                raise StopIteration
        self._tgt_key = self._curr_key
        return self._tgt_key, _GroupByIter(self, self._tgt_key)

    def _advance(self) -> None:
        try:
            self._curr_val = next(self._it)
        except StopIteration:
            self._done = True
            self._curr_key = _SENTINEL
            return
        self._curr_key = self._key(self._curr_val)


class _GroupByIter:
    def __init__(self, parent: _GroupBy, target_key: Any) -> None:
        self._parent = parent
        self._target_key = target_key

    def __iter__(self):
        return self

    def __next__(self):
        parent = self._parent
        if parent._done or parent._curr_key != self._target_key:
            raise StopIteration
        value = parent._curr_val
        parent._advance()
        return value


def groupby(iterable: Iterable[T], key: Callable[[T], Any] | None = None):
    return _GroupBy(iterable, key)


class _TeeData:
    def __init__(self, iterable: Iterable[T]) -> None:
        self._it = iter(iterable)
        self._values: list[Any] = []
        self._done = False

    def get(self, index: int) -> Any:
        if index < len(self._values):
            return self._values[index]
        if self._done:
            raise StopIteration
        try:
            value = next(self._it)
        except StopIteration:
            self._done = True
            raise
        self._values.append(value)
        return value


class _TeeIterator:
    def __init__(self, data: _TeeData[T]) -> None:
        self._data = data
        self._index = 0

    def __iter__(self):
        return self

    def __next__(self):
        value = self._data.get(self._index)
        self._index += 1
        return value


def tee(iterable: Iterable[T], n: int = 2) -> tuple[Iterator[T], ...]:
    if n <= 0:
        return ()
    data = _TeeData(iterable)
    return tuple(_TeeIterator(data) for _ in range(n))
