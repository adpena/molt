"""Binary search helpers for Molt."""

from __future__ import annotations

from typing import Any, Callable, TypeVar

__all__ = [
    "bisect_left",
    "bisect_right",
    "bisect",
    "insort_left",
    "insort_right",
    "insort",
]

T = TypeVar("T")


def _coerce_index(value: Any) -> tuple[bool, int, str | None]:
    if isinstance(value, int):
        return True, value, None
    try:
        idx = value.__index__()
    except AttributeError:
        return (
            False,
            0,
            f"'{type(value).__name__}' object cannot be interpreted as an integer",
        )
    if not isinstance(idx, int):
        return (
            False,
            0,
            f"__index__ returned non-int (type {type(idx).__name__})",
        )
    return True, idx, None


# Return errors for the caller to raise to avoid delayed exception propagation.
def _normalize_bounds(
    lo: Any, hi: Any | None, size: int
) -> tuple[int, int, Exception | None]:
    ok, lo_idx, err = _coerce_index(lo)
    if not ok:
        return 0, 0, TypeError(err)
    if lo_idx < 0:
        return 0, 0, ValueError("lo must be non-negative")
    if hi is None:
        return lo_idx, size, None
    ok, hi_idx, err = _coerce_index(hi)
    if not ok:
        return 0, 0, TypeError(err)
    if hi_idx > size:
        return 0, 0, IndexError("list index out of range")
    return lo_idx, hi_idx, None


def bisect_left(
    a: Any,
    x: T,
    lo: int = 0,
    hi: int | None = None,
    *,
    key: Callable[[T], Any] | None = None,
) -> int:
    lo_idx, hi_idx, err = _normalize_bounds(lo, hi, len(a))
    if err is not None:
        raise err
    if key is None:
        while lo_idx < hi_idx:
            mid = (lo_idx + hi_idx) // 2
            if a[mid] < x:
                lo_idx = mid + 1
            else:
                hi_idx = mid
        return lo_idx
    while lo_idx < hi_idx:
        mid = (lo_idx + hi_idx) // 2
        if key(a[mid]) < x:
            lo_idx = mid + 1
        else:
            hi_idx = mid
    return lo_idx


def bisect_right(
    a: Any,
    x: T,
    lo: int = 0,
    hi: int | None = None,
    *,
    key: Callable[[T], Any] | None = None,
) -> int:
    lo_idx, hi_idx, err = _normalize_bounds(lo, hi, len(a))
    if err is not None:
        raise err
    if key is None:
        while lo_idx < hi_idx:
            mid = (lo_idx + hi_idx) // 2
            if x < a[mid]:
                hi_idx = mid
            else:
                lo_idx = mid + 1
        return lo_idx
    while lo_idx < hi_idx:
        mid = (lo_idx + hi_idx) // 2
        if x < key(a[mid]):
            hi_idx = mid
        else:
            lo_idx = mid + 1
    return lo_idx


def insort_left(
    a: list[T],
    x: T,
    lo: int = 0,
    hi: int | None = None,
    *,
    key: Callable[[T], Any] | None = None,
) -> None:
    lo_idx, hi_idx, err = _normalize_bounds(lo, hi, len(a))
    if err is not None:
        raise err
    if key is None:
        pos = bisect_left(a, x, lo_idx, hi_idx)
    else:
        pos = bisect_left(a, key(x), lo_idx, hi_idx, key=key)
    a.insert(pos, x)


def insort_right(
    a: list[T],
    x: T,
    lo: int = 0,
    hi: int | None = None,
    *,
    key: Callable[[T], Any] | None = None,
) -> None:
    lo_idx, hi_idx, err = _normalize_bounds(lo, hi, len(a))
    if err is not None:
        raise err
    if key is None:
        pos = bisect_right(a, x, lo_idx, hi_idx)
    else:
        pos = bisect_right(a, key(x), lo_idx, hi_idx, key=key)
    a.insert(pos, x)


bisect = bisect_right
insort = insort_right
