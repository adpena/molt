"""`molt.typed_cache` keeps functools' cache behavior under a typed signature.

The typing half is enforced by `ty check src` (the ParamSpec protocol checks
every call against the wrapped parameters); these cases pin that the runtime
object is the same LRU cache, with its controls.
"""

from __future__ import annotations

from molt.typed_cache import typed_cache


def test_typed_cache_memoizes_and_exposes_the_lru_cache_controls() -> None:
    calls: list[int] = []

    @typed_cache(maxsize=2)
    def square(value: int) -> int:
        calls.append(value)
        return value * value

    assert [square(3), square(3), square(4)] == [9, 9, 16]
    assert calls == [3, 4]
    info = square.cache_info()
    assert (info.hits, info.misses, info.maxsize, info.currsize) == (1, 2, 2, 2)
    assert square.cache_parameters()["maxsize"] == 2
    square(5)
    assert square.cache_info().currsize == 2
    square.cache_clear()
    assert square.cache_info().currsize == 0


def test_an_unbounded_typed_cache_never_evicts() -> None:
    @typed_cache(maxsize=None)
    def identity(value: str) -> str:
        return value

    for value in ("a", "b", "c", "a", "b", "c"):
        identity(value)
    info = identity.cache_info()
    assert (info.hits, info.misses, info.maxsize, info.currsize) == (3, 3, None, 3)
