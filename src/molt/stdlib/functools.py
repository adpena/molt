"""Deterministic functools shim for Molt."""

from __future__ import annotations

from typing import Any, Callable, Iterable

__all__ = [
    "lru_cache",
    "partial",
    "reduce",
    "update_wrapper",
    "wraps",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL1): add full functools surface
# (namedtuple cache_info, singledispatch, total_ordering, cmp_to_key).

WRAPPER_ASSIGNMENTS = (
    "__module__",
    "__name__",
    "__qualname__",
    "__doc__",
    "__annotations__",
)
WRAPPER_UPDATES = ("__dict__",)


def update_wrapper(
    wrapper: Callable[..., Any],
    wrapped: Callable[..., Any],
    assigned: Iterable[str] = WRAPPER_ASSIGNMENTS,
    updated: Iterable[str] = WRAPPER_UPDATES,
) -> Callable[..., Any]:
    for attr in assigned:
        try:
            value = getattr(wrapped, attr)
        except Exception:
            continue
        try:
            setattr(wrapper, attr, value)
        except Exception:
            continue
    for attr in updated:
        try:
            target = getattr(wrapper, attr)
            source = getattr(wrapped, attr)
        except Exception:
            continue
        try:
            target.update(source)
        except Exception:
            continue
    try:
        setattr(wrapper, "__wrapped__", wrapped)
    except Exception:
        pass
    return wrapper


class _Wraps:
    def __init__(
        self,
        wrapped: Callable[..., Any],
        assigned: Iterable[str],
        updated: Iterable[str],
    ) -> None:
        self._wrapped = wrapped
        self._assigned = assigned
        self._updated = updated

    def __call__(self, wrapper: Callable[..., Any]) -> Callable[..., Any]:
        return update_wrapper(
            wrapper,
            self._wrapped,
            assigned=self._assigned,
            updated=self._updated,
        )


def wraps(
    wrapped: Callable[..., Any],
    assigned: Iterable[str] = WRAPPER_ASSIGNMENTS,
    updated: Iterable[str] = WRAPPER_UPDATES,
) -> Callable[[Callable[..., Any]], Callable[..., Any]]:
    return _Wraps(wrapped, assigned, updated)


class _Partial:
    def __init__(
        self,
        func: Callable[..., Any],
        args: tuple[Any, ...],
        keywords: dict[str, Any] | None,
    ) -> None:
        if func is None:
            raise TypeError("partial() requires a callable")
        self.func = func
        self.args = args
        self.keywords = keywords

    def __call__(self, *args: Any, **kwargs: Any) -> Any:
        if self.keywords:
            merged = dict(self.keywords)
            merged.update(kwargs)
        else:
            merged = kwargs
        return self.func(*self.args, *args, **merged)

    def __repr__(self) -> str:
        if self.keywords:
            return (
                "functools.partial("
                + repr(self.func)
                + ", "
                + repr(self.args)
                + ", "
                + repr(self.keywords)
                + ")"
            )
        return "functools.partial(" + repr(self.func) + ", " + repr(self.args) + ")"


def partial(func: Callable[..., Any], *args: Any, **keywords: Any) -> _Partial:
    return _Partial(func, args, keywords or None)


class _CacheInfo:
    def __init__(
        self, hits: int, misses: int, maxsize: int | None, currsize: int
    ) -> None:
        self.hits = hits
        self.misses = misses
        self.maxsize = maxsize
        self.currsize = currsize

    def __iter__(self):
        return iter((self.hits, self.misses, self.maxsize, self.currsize))

    def __repr__(self) -> str:
        return (
            "CacheInfo(hits="
            + repr(self.hits)
            + ", misses="
            + repr(self.misses)
            + ", maxsize="
            + repr(self.maxsize)
            + ", currsize="
            + repr(self.currsize)
            + ")"
        )


_kwd_mark = object()


class _LruCacheWrapper:
    def __init__(
        self, func: Callable[..., Any], maxsize: int | None, typed: bool
    ) -> None:
        self._func = func
        self._maxsize = maxsize
        self._typed = typed
        self._cache: dict[tuple[Any, ...], Any] = {}
        self._order: list[tuple[Any, ...]] = []
        self._hits = 0
        self._misses = 0

    def __call__(self, *args: Any, **kwargs: Any) -> Any:
        if self._maxsize == 0:
            self._misses += 1
            return self._func(*args, **kwargs)
        key = _make_lru_key(args, kwargs, self._typed)
        if key in self._cache:
            self._hits += 1
            if self._maxsize is not None:
                try:
                    self._order.remove(key)
                except ValueError:
                    pass
                self._order.append(key)
            return self._cache[key]
        self._misses += 1
        result = self._func(*args, **kwargs)
        self._cache[key] = result
        if self._maxsize is not None:
            self._order.append(key)
            if len(self._order) > self._maxsize:
                oldest = self._order.pop(0)
                self._cache.pop(oldest, None)
        return result

    def cache_info(self) -> _CacheInfo:
        return _CacheInfo(self._hits, self._misses, self._maxsize, len(self._cache))

    def cache_clear(self) -> None:
        self._cache.clear()
        self._order.clear()
        self._hits = 0
        self._misses = 0

    def cache_parameters(self) -> dict[str, Any]:
        return {"maxsize": self._maxsize, "typed": self._typed}


def _make_lru_key(
    args: tuple[Any, ...], kwargs: dict[str, Any], typed: bool
) -> tuple[Any, ...]:
    if kwargs:
        items: list[tuple[str, Any]] = []
        for item in kwargs.items():
            items.append(item)
        key: tuple[Any, ...] = args + (_kwd_mark,) + tuple(items)
    else:
        key = args
    if typed:
        types: list[type[Any]] = []
        for val in args:
            types.append(type(val))
        if kwargs:
            for _, val in kwargs.items():
                types.append(type(val))
        key = key + tuple(types)
    return key


class _LruCacheFactory:
    def __init__(self, maxsize: int | None, typed: bool) -> None:
        self._maxsize = maxsize
        self._typed = typed

    def __call__(self, func: Callable[..., Any]) -> Callable[..., Any]:
        wrapper = _LruCacheWrapper(func, self._maxsize, self._typed)
        # TODO(stdlib-compat, owner:stdlib, milestone:SL1): restore update_wrapper.
        return wrapper


def lru_cache(maxsize: int | None = 128, typed: bool = False):
    if callable(maxsize) and typed is False:
        func = maxsize
        wrapper = _LruCacheWrapper(func, 128, False)
        # TODO(stdlib-compat, owner:stdlib, milestone:SL1): restore update_wrapper.
        return wrapper
    return _LruCacheFactory(maxsize, typed)


def reduce(
    function: Callable[[Any, Any], Any],
    iterable: Iterable[Any],
    initializer: Any = _kwd_mark,
) -> Any:
    it = iter(iterable)
    if initializer is _kwd_mark:
        try:
            value = next(it)
        except StopIteration as exc:
            raise TypeError("reduce() of empty sequence with no initial value") from exc
    else:
        value = initializer
    for item in it:
        value = function(value, item)
    return value
