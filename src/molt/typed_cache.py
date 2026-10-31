"""A memoizing decorator that keeps the function's signature for the type checker.

`functools.lru_cache` returns `_lru_cache_wrapper`, whose `__call__` takes
`*args: Hashable`, so `ty` stops checking every call to a cached function: a
cached function typed for `ArtifactTarget` accepted the string
"wasm32-wasip1" unseen. `typed_cache` returns the same CPython cache under a
ParamSpec protocol, so calls are checked against the original parameters.
Raw `functools.lru_cache` and `functools.cache` are banned in `src/molt`
(ruff TID251); the compiled stdlib under `src/molt/stdlib` is exempt.
"""

from __future__ import annotations

from collections.abc import Callable
from functools import lru_cache
from typing import NamedTuple, ParamSpec, Protocol, TypeVar, cast

P = ParamSpec("P")
R = TypeVar("R")
R_co = TypeVar("R_co", covariant=True)


class CacheInfo(NamedTuple):
    hits: int
    misses: int
    maxsize: int | None
    currsize: int


class CachedFunction(Protocol[P, R_co]):
    def __call__(self, *args: P.args, **kwargs: P.kwargs) -> R_co: ...

    def cache_clear(self) -> None: ...

    def cache_info(self) -> CacheInfo: ...

    def cache_parameters(self) -> dict[str, int | bool | None]: ...


def typed_cache(
    *, maxsize: int | None
) -> Callable[[Callable[P, R]], CachedFunction[P, R]]:
    """`functools.lru_cache(maxsize=maxsize)`, typed by the wrapped signature."""

    def decorate(function: Callable[P, R]) -> CachedFunction[P, R]:
        return cast("CachedFunction[P, R]", lru_cache(maxsize=maxsize)(function))

    return decorate
