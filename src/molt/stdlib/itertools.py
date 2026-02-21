"""Iterator helpers for Molt.

Iterator helpers are backed by runtime intrinsics; missing intrinsics are a hard error.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


from typing import Any, Iterable, Iterator, TYPE_CHECKING, TypeVar


if TYPE_CHECKING:
    pass

__all__ = [
    "accumulate",
    "batched",
    "chain",
    "combinations",
    "combinations_with_replacement",
    "compress",
    "count",
    "cycle",
    "dropwhile",
    "filterfalse",
    "groupby",
    "islice",
    "pairwise",
    "permutations",
    "product",
    "repeat",
    "starmap",
    "takewhile",
    "tee",
    "zip_longest",
]

T = TypeVar("T")

_MOLT_KWD_MARK = _require_intrinsic("molt_itertools_kwd_mark", globals())
_MOLT_CHAIN = _require_intrinsic("molt_itertools_chain", globals())
_MOLT_CHAIN_FROM_ITERABLE = _require_intrinsic(
    "molt_itertools_chain_from_iterable", globals()
)
_MOLT_ISLICE = _require_intrinsic("molt_itertools_islice", globals())
_MOLT_REPEAT = _require_intrinsic("molt_itertools_repeat", globals())
_MOLT_COUNT = _require_intrinsic("molt_itertools_count", globals())
_MOLT_CYCLE = _require_intrinsic("molt_itertools_cycle", globals())
_MOLT_ACCUMULATE = _require_intrinsic("molt_itertools_accumulate", globals())
_MOLT_BATCHED = _require_intrinsic("molt_itertools_batched", globals())
_MOLT_COMPRESS = _require_intrinsic("molt_itertools_compress", globals())
_MOLT_COMBINATIONS_WITH_REPLACEMENT = _require_intrinsic(
    "molt_itertools_combinations_with_replacement", globals()
)
_MOLT_DROPWHILE = _require_intrinsic("molt_itertools_dropwhile", globals())
_MOLT_FILTERFALSE = _require_intrinsic("molt_itertools_filterfalse", globals())
_MOLT_PAIRWISE = _require_intrinsic("molt_itertools_pairwise", globals())
_MOLT_PRODUCT = _require_intrinsic("molt_itertools_product", globals())
_MOLT_PERMUTATIONS = _require_intrinsic("molt_itertools_permutations", globals())
_MOLT_COMBINATIONS = _require_intrinsic("molt_itertools_combinations", globals())
_MOLT_GROUPBY = _require_intrinsic("molt_itertools_groupby", globals())
_MOLT_STARMAP = _require_intrinsic("molt_itertools_starmap", globals())
_MOLT_TAKEWHILE = _require_intrinsic("molt_itertools_takewhile", globals())
_MOLT_TEE = _require_intrinsic("molt_itertools_tee", globals())
_MOLT_ZIP_LONGEST = _require_intrinsic("molt_itertools_zip_longest", globals())

_MISSING = _MOLT_KWD_MARK()


def chain(*iterables: Iterable[T]) -> Iterator[T]:
    return _MOLT_CHAIN(iterables)


def _chain_from_iterable(iterables: Iterable[Iterable[T]]) -> Iterator[T]:
    return _MOLT_CHAIN_FROM_ITERABLE(iterables)


chain.from_iterable = _chain_from_iterable  # type: ignore[attr-defined]


def islice(iterable: Iterable[T], /, *args: Any) -> Iterator[T]:
    if len(args) == 1:
        start = args[0]
        stop = _MISSING
        step = _MISSING
    elif len(args) == 2:
        start, stop = args
        step = _MISSING
    elif len(args) == 3:
        start, stop, step = args
    else:
        raise TypeError("islice() takes 2 to 4 arguments")
    return _MOLT_ISLICE(iterable, start, stop, step)


def repeat(obj: T, times: int | None = None) -> Iterator[T]:
    return _MOLT_REPEAT(obj, times)


def count(start: Any = 0, step: Any = 1) -> Iterator[Any]:
    return _MOLT_COUNT(start, step)


def cycle(iterable: Iterable[T]) -> Iterator[T]:
    return _MOLT_CYCLE(iterable)


def accumulate(
    iterable: Iterable[T], func: Any = None, *, initial: Any = _MISSING
) -> Iterator[Any]:
    return _MOLT_ACCUMULATE(iterable, func, initial)


def batched(
    iterable: Iterable[T], n: Any, *, strict: Any = False
) -> Iterator[tuple[T, ...]]:
    return _MOLT_BATCHED(iterable, n, strict)


def pairwise(iterable: Iterable[T]) -> Iterator[tuple[T, T]]:
    return _MOLT_PAIRWISE(iterable)


class compress:
    def __new__(cls, data: Iterable[T], selectors: Iterable[Any]) -> Iterator[T]:
        return _MOLT_COMPRESS(data, selectors)


class dropwhile:
    def __new__(cls, predicate: Any, iterable: Iterable[T]) -> Iterator[T]:
        return _MOLT_DROPWHILE(predicate, iterable)


class filterfalse:
    def __new__(cls, predicate: Any, iterable: Iterable[T]) -> Iterator[T]:
        return _MOLT_FILTERFALSE(predicate, iterable)


def product(*iterables: Iterable[T], repeat: Any = 1) -> Iterator[tuple[Any, ...]]:
    return _MOLT_PRODUCT(iterables, repeat)


def permutations(
    iterable: Iterable[T], r: Any | None = None
) -> Iterator[tuple[T, ...]]:
    return _MOLT_PERMUTATIONS(iterable, r)


def combinations(iterable: Iterable[T], r: Any) -> Iterator[tuple[T, ...]]:
    return _MOLT_COMBINATIONS(iterable, r)


def combinations_with_replacement(
    iterable: Iterable[T], r: Any
) -> Iterator[tuple[T, ...]]:
    return _MOLT_COMBINATIONS_WITH_REPLACEMENT(iterable, r)


def groupby(iterable: Iterable[T], key: Any | None = None) -> Any:
    return _MOLT_GROUPBY(iterable, key)


class starmap:
    def __new__(cls, function: Any, iterable: Iterable[Any]) -> Iterator[Any]:
        return _MOLT_STARMAP(function, iterable)


class takewhile:
    def __new__(cls, predicate: Any, iterable: Iterable[T]) -> Iterator[T]:
        return _MOLT_TAKEWHILE(predicate, iterable)


def tee(iterable: Iterable[T], n: Any = 2):
    return _MOLT_TEE(iterable, n)


class zip_longest:
    def __new__(
        cls, *iterables: Iterable[Any], fillvalue: Any = None
    ) -> Iterator[tuple[Any, ...]]:
        return _MOLT_ZIP_LONGEST(iterables, fillvalue)
