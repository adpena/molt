"""Binary search helpers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from typing import Any, Callable, TypeVar

_MOLT_BISECT_LEFT = _require_intrinsic("molt_bisect_left", globals())
_MOLT_BISECT_RIGHT = _require_intrinsic("molt_bisect_right", globals())
_MOLT_INSORT_LEFT = _require_intrinsic("molt_insort_left", globals())
_MOLT_INSORT_RIGHT = _require_intrinsic("molt_insort_right", globals())


__all__ = [
    "bisect_left",
    "bisect_right",
    "bisect",
    "insort_left",
    "insort_right",
    "insort",
]

T = TypeVar("T")


def bisect_left(
    a: Any,
    x: T,
    lo: int = 0,
    hi: int | None = None,
    *,
    key: Callable[[T], Any] | None = None,
) -> int:
    return _MOLT_BISECT_LEFT(a, x, lo, hi, key)


def bisect_right(
    a: Any,
    x: T,
    lo: int = 0,
    hi: int | None = None,
    *,
    key: Callable[[T], Any] | None = None,
) -> int:
    return _MOLT_BISECT_RIGHT(a, x, lo, hi, key)


def insort_left(
    a: list[T],
    x: T,
    lo: int = 0,
    hi: int | None = None,
    *,
    key: Callable[[T], Any] | None = None,
) -> None:
    _MOLT_INSORT_LEFT(a, x, lo, hi, key)


def insort_right(
    a: list[T],
    x: T,
    lo: int = 0,
    hi: int | None = None,
    *,
    key: Callable[[T], Any] | None = None,
) -> None:
    _MOLT_INSORT_RIGHT(a, x, lo, hi, key)


bisect = bisect_right
insort = insort_right
