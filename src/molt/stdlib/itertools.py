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

_MOLT_KWD_MARK = _require_intrinsic("molt_itertools_kwd_mark")
_MOLT_CHAIN = _require_intrinsic("molt_itertools_chain")
_MOLT_CHAIN_FROM_ITERABLE = _require_intrinsic(
    "molt_itertools_chain_from_iterable"
)
_MOLT_ISLICE = _require_intrinsic("molt_itertools_islice")
_MOLT_REPEAT = _require_intrinsic("molt_itertools_repeat")
_MOLT_COUNT = _require_intrinsic("molt_itertools_count")
_MOLT_ACCUMULATE = _require_intrinsic("molt_itertools_accumulate")
_MOLT_BATCHED = _require_intrinsic("molt_itertools_batched")
_MOLT_PRODUCT = _require_intrinsic("molt_itertools_product")
_MOLT_PERMUTATIONS = _require_intrinsic("molt_itertools_permutations")
_MOLT_GROUPBY = _require_intrinsic("molt_itertools_groupby")
_MOLT_TEE = _require_intrinsic("molt_itertools_tee")
_MOLT_ZIP_LONGEST = _require_intrinsic("molt_itertools_zip_longest")

_MISSING = _MOLT_KWD_MARK()

# --- Direct intrinsic bindings (no Python wrapper overhead) ---

cycle = _require_intrinsic("molt_itertools_cycle")
pairwise = _require_intrinsic("molt_itertools_pairwise")
combinations = _require_intrinsic("molt_itertools_combinations")
combinations_with_replacement = _require_intrinsic(
    "molt_itertools_combinations_with_replacement"
)

# Classes whose __new__ is a pure forwarding shim — bind intrinsic directly.
# CPython exposes these as types, but Molt callers use them as callables;
# the intrinsic returns an iterator, preserving the call-site contract.
compress = _require_intrinsic("molt_itertools_compress")
dropwhile = _require_intrinsic("molt_itertools_dropwhile")
filterfalse = _require_intrinsic("molt_itertools_filterfalse")
starmap = _require_intrinsic("molt_itertools_starmap")
takewhile = _require_intrinsic("molt_itertools_takewhile")


# --- Retained wrappers (argument adaptation or Python logic required) ---


def chain(*iterables: Iterable[T], _chain_intrinsic=_MOLT_CHAIN) -> Iterator[T]:
    return _chain_intrinsic(iterables)


def _chain_from_iterable(
    iterables: Iterable[Iterable[T]],
    _chain_from_iterable_intrinsic=_MOLT_CHAIN_FROM_ITERABLE,
) -> Iterator[T]:
    return _chain_from_iterable_intrinsic(iterables)


chain.from_iterable = _chain_from_iterable  # type: ignore[attr-defined]


def islice(
    iterable: Iterable[T],
    /,
    start_or_stop: Any = _MISSING,
    stop: Any = _MISSING,
    step: Any = _MISSING,
    _islice_intrinsic=_MOLT_ISLICE,
    _missing=_MISSING,
) -> Iterator[T]:
    if start_or_stop is _missing:
        raise TypeError("islice() takes 2 to 4 arguments")
    if stop is _missing and step is _missing:
        # islice(iterable, stop) — single positional arg is the stop value
        return _islice_intrinsic(iterable, start_or_stop, _missing, _missing)
    # islice(iterable, start, stop[, step])
    return _islice_intrinsic(iterable, start_or_stop, stop, step)


def repeat(obj: T, times: int | None = None, _repeat_intrinsic=_MOLT_REPEAT) -> Iterator[T]:
    return _repeat_intrinsic(obj, times)


def count(start: Any = 0, step: Any = 1, _count_intrinsic=_MOLT_COUNT) -> Iterator[Any]:
    return _count_intrinsic(start, step)


def accumulate(
    iterable: Iterable[T],
    func: Any = None,
    *,
    initial: Any = _MISSING,
    _accumulate_intrinsic=_MOLT_ACCUMULATE,
) -> Iterator[Any]:
    return _accumulate_intrinsic(iterable, func, initial)


def batched(
    iterable: Iterable[T],
    n: Any,
    *,
    strict: Any = False,
    _batched_intrinsic=_MOLT_BATCHED,
) -> Iterator[tuple[T, ...]]:
    return _batched_intrinsic(iterable, n, strict)


def product(
    *iterables: Iterable[T], repeat: Any = 1, _product_intrinsic=_MOLT_PRODUCT
) -> Iterator[tuple[Any, ...]]:
    return _product_intrinsic(iterables, repeat)


def permutations(
    iterable: Iterable[T],
    r: Any | None = None,
    _permutations_intrinsic=_MOLT_PERMUTATIONS,
) -> Iterator[tuple[T, ...]]:
    return _permutations_intrinsic(iterable, r)


def groupby(
    iterable: Iterable[T], key: Any | None = None, _groupby_intrinsic=_MOLT_GROUPBY
) -> Any:
    return _groupby_intrinsic(iterable, key)


def tee(iterable: Iterable[T], n: Any = 2, _tee_intrinsic=_MOLT_TEE):
    return _tee_intrinsic(iterable, n)


class zip_longest:
    def __new__(
        cls,
        *iterables: Iterable[Any],
        fillvalue: Any = None,
        _zip_longest_intrinsic=_MOLT_ZIP_LONGEST,
    ) -> Iterator[tuple[Any, ...]]:
        return _zip_longest_intrinsic(iterables, fillvalue)


for _name in (
    "_MOLT_KWD_MARK",
    "_MOLT_CHAIN",
    "_MOLT_CHAIN_FROM_ITERABLE",
    "_MOLT_ISLICE",
    "_MOLT_REPEAT",
    "_MOLT_COUNT",
    "_MOLT_ACCUMULATE",
    "_MOLT_BATCHED",
    "_MOLT_PRODUCT",
    "_MOLT_PERMUTATIONS",
    "_MOLT_GROUPBY",
    "_MOLT_TEE",
    "_MOLT_ZIP_LONGEST",
    "_MISSING",
):
    globals().pop(_name, None)

globals().pop("_require_intrinsic", None)
