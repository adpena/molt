# Shim churn audit: 6 intrinsic-direct / 20 total exports
"""Iterator helpers for Molt.

Iterator helpers are backed by runtime intrinsics; missing intrinsics are a hard error.
Pure-forwarding shims eliminated per MOL-215 where argument signatures permit.
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
_MOLT_ACCUMULATE = _require_intrinsic("molt_itertools_accumulate", globals())
_MOLT_BATCHED = _require_intrinsic("molt_itertools_batched", globals())
_MOLT_PRODUCT = _require_intrinsic("molt_itertools_product", globals())
_MOLT_PERMUTATIONS = _require_intrinsic("molt_itertools_permutations", globals())
_MOLT_GROUPBY = _require_intrinsic("molt_itertools_groupby", globals())
_MOLT_TEE = _require_intrinsic("molt_itertools_tee", globals())
_MOLT_ZIP_LONGEST = _require_intrinsic("molt_itertools_zip_longest", globals())

_MISSING = _MOLT_KWD_MARK()

# --- Direct intrinsic bindings (no Python wrapper overhead) ---

cycle = _require_intrinsic("molt_itertools_cycle", globals())
pairwise = _require_intrinsic("molt_itertools_pairwise", globals())
combinations = _require_intrinsic("molt_itertools_combinations", globals())
combinations_with_replacement = _require_intrinsic(
    "molt_itertools_combinations_with_replacement", globals()
)

# Classes whose __new__ is a pure forwarding shim — bind intrinsic directly.
# CPython exposes these as types, but Molt callers use them as callables;
# the intrinsic returns an iterator, preserving the call-site contract.
compress = _require_intrinsic("molt_itertools_compress", globals())
dropwhile = _require_intrinsic("molt_itertools_dropwhile", globals())
filterfalse = _require_intrinsic("molt_itertools_filterfalse", globals())
starmap = _require_intrinsic("molt_itertools_starmap", globals())
takewhile = _require_intrinsic("molt_itertools_takewhile", globals())


# --- Retained wrappers (argument adaptation or Python logic required) ---


def chain(*iterables: Iterable[T]) -> Iterator[T]:
    return _MOLT_CHAIN(iterables)


def _chain_from_iterable(iterables: Iterable[Iterable[T]]) -> Iterator[T]:
    return _MOLT_CHAIN_FROM_ITERABLE(iterables)


chain.from_iterable = _chain_from_iterable  # type: ignore[attr-defined]


def islice(
    iterable: Iterable[T],
    /,
    start_or_stop: Any = _MISSING,
    stop: Any = _MISSING,
    step: Any = _MISSING,
) -> Iterator[T]:
    if start_or_stop is _MISSING:
        raise TypeError("islice() takes 2 to 4 arguments")
    if stop is _MISSING and step is _MISSING:
        # islice(iterable, stop) — single positional arg is the stop value
        return _MOLT_ISLICE(iterable, start_or_stop, _MISSING, _MISSING)
    # islice(iterable, start, stop[, step])
    return _MOLT_ISLICE(iterable, start_or_stop, stop, step)


def repeat(obj: T, times: int | None = None) -> Iterator[T]:
    return _MOLT_REPEAT(obj, times)


def count(start: Any = 0, step: Any = 1) -> Iterator[Any]:
    return _MOLT_COUNT(start, step)


def accumulate(
    iterable: Iterable[T], func: Any = None, *, initial: Any = _MISSING
) -> Iterator[Any]:
    return _MOLT_ACCUMULATE(iterable, func, initial)


def batched(
    iterable: Iterable[T], n: Any, *, strict: Any = False
) -> Iterator[tuple[T, ...]]:
    return _MOLT_BATCHED(iterable, n, strict)


def product(*iterables: Iterable[T], repeat: Any = 1) -> Iterator[tuple[Any, ...]]:
    return _MOLT_PRODUCT(iterables, repeat)


def permutations(
    iterable: Iterable[T], r: Any | None = None
) -> Iterator[tuple[T, ...]]:
    return _MOLT_PERMUTATIONS(iterable, r)


def groupby(iterable: Iterable[T], key: Any | None = None) -> Any:
    return _MOLT_GROUPBY(iterable, key)


def tee(iterable: Iterable[T], n: Any = 2):
    return _MOLT_TEE(iterable, n)


class zip_longest:
    def __new__(
        cls, *iterables: Iterable[Any], fillvalue: Any = None
    ) -> Iterator[tuple[Any, ...]]:
        return _MOLT_ZIP_LONGEST(iterables, fillvalue)
