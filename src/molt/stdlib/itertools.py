"""Iterator helpers for Molt."""

from __future__ import annotations

from typing import Iterable, Iterator, TypeVar

__all__ = [
    "chain",
    "islice",
    "repeat",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL1): add product, permutations,
# combinations, groupby, tee, and cycle.

T = TypeVar("T")


def chain(first: Iterable[T], second: Iterable[T]) -> Iterator[T]:
    # TODO(stdlib-compat, owner:stdlib, milestone:SL1): support variadic iterables without eager collection.
    out: list[T] = []
    for item in first:
        out.append(item)
    for item in second:
        out.append(item)
    return iter(out)


def islice(
    iterable: Iterable[T],
    start: int,
    stop: int | None = None,
    step: int = 1,
) -> Iterator[T]:
    if stop is None:
        stop = start
        start = 0
    if step <= 0:
        raise ValueError("islice() step must be a positive integer")
    # TODO(stdlib-compat, owner:stdlib, milestone:SL1): avoid eager materialization.
    idx = 0
    out: list[T] = []
    for item in iterable:
        if idx >= stop:
            break
        if idx >= start and (idx - start) % step == 0:
            out.append(item)
        idx += 1
    return iter(out)


def repeat(obj: T, times: int | None = None) -> Iterator[T]:
    if times is None:
        raise NotImplementedError("repeat() without times is not supported yet")
    out: list[T] = []
    for _ in range(times):
        out.append(obj)
    return iter(out)
