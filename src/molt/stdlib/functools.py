"""Deterministic functools shim for Molt."""

from __future__ import annotations

from typing import Any, Callable, Iterable, cast

__all__ = [
    "cmp_to_key",
    "lru_cache",
    "partial",
    "reduce",
    "total_ordering",
    "update_wrapper",
    "wraps",
]

# TODO(stdlib-compat, owner:stdlib, milestone:SL1): add full functools surface
# (namedtuple cache_info, singledispatch).

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
    wrapper_obj = cast(Any, wrapper)
    wrapped_obj = cast(Any, wrapped)
    if assigned is WRAPPER_ASSIGNMENTS:
        try:
            wrapper_obj.__module__ = wrapped_obj.__module__
        except Exception:
            pass
        try:
            wrapper_obj.__name__ = wrapped_obj.__name__
        except Exception:
            pass
        try:
            wrapper_obj.__qualname__ = wrapped_obj.__qualname__
        except Exception:
            pass
        try:
            wrapper_obj.__doc__ = wrapped_obj.__doc__
        except Exception:
            pass
        try:
            wrapper_obj.__annotations__ = wrapped_obj.__annotations__
        except Exception:
            pass
    else:
        for attr in assigned:
            try:
                value = getattr(wrapped, attr)
            except Exception:
                continue
            try:
                setattr(wrapper, attr, value)
            except Exception:
                continue
    if updated is WRAPPER_UPDATES:
        try:
            for key, value in wrapped_obj.__dict__.items():
                if isinstance(key, str) and key.startswith("__molt_"):
                    continue
                wrapper_obj.__dict__[key] = value
        except Exception:
            pass
    else:
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
        wrapper_obj.__wrapped__ = wrapped_obj
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


def cmp_to_key(mycmp):
    def key(obj: Any) -> _CmpKey:
        return _CmpKey(obj, mycmp)

    return key


class _CmpKey:
    __slots__ = ("obj", "_cmp")

    def __init__(self, obj: Any, cmp: Callable[[Any, Any], int]) -> None:
        self.obj = obj
        self._cmp = cmp

    def __lt__(self, other: object) -> bool:
        if type(other) is not type(self):
            return NotImplemented
        other_key = cast(_CmpKey, other)
        return self._cmp(self.obj, other_key.obj) < 0

    def __le__(self, other: object) -> bool:
        if type(other) is not type(self):
            return NotImplemented
        other_key = cast(_CmpKey, other)
        return self._cmp(self.obj, other_key.obj) <= 0

    def __gt__(self, other: object) -> bool:
        if type(other) is not type(self):
            return NotImplemented
        other_key = cast(_CmpKey, other)
        return self._cmp(self.obj, other_key.obj) > 0

    def __ge__(self, other: object) -> bool:
        if type(other) is not type(self):
            return NotImplemented
        other_key = cast(_CmpKey, other)
        return self._cmp(self.obj, other_key.obj) >= 0

    def __eq__(self, other: object) -> bool:
        if type(other) is not type(self):
            return False
        other_key = cast(_CmpKey, other)
        return self._cmp(self.obj, other_key.obj) == 0

    def __ne__(self, other: object) -> bool:
        if type(other) is not type(self):
            return True
        other_key = cast(_CmpKey, other)
        return self._cmp(self.obj, other_key.obj) != 0

    __hash__ = None


def total_ordering(cls):
    convert = {
        "__lt__": [
            ("__gt__", lambda self, other: other < self),
            ("__le__", lambda self, other: not other < self),
            ("__ge__", lambda self, other: not self < other),
        ],
        "__le__": [
            ("__ge__", lambda self, other: other <= self),
            ("__lt__", lambda self, other: not other <= self),
            ("__gt__", lambda self, other: not self <= other),
        ],
        "__gt__": [
            ("__lt__", lambda self, other: other > self),
            ("__ge__", lambda self, other: not other > self),
            ("__le__", lambda self, other: not self > other),
        ],
        "__ge__": [
            ("__le__", lambda self, other: other >= self),
            ("__gt__", lambda self, other: not other >= self),
            ("__lt__", lambda self, other: not self >= other),
        ],
    }
    root = None
    for op_name in ("__lt__", "__le__", "__gt__", "__ge__"):
        if op_name in cls.__dict__:
            root = op_name
            break
    if root is None:
        raise ValueError(
            "total_ordering requires at least one ordering operation: < <= > >="
        )
    for opname, opfunc in convert[root]:
        if opname not in cls.__dict__:
            setattr(cls, opname, opfunc)
    return cls


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
        return update_wrapper(wrapper, func)


def lru_cache(maxsize: int | None = 128, typed: bool = False):
    if callable(maxsize) and typed is False:
        func = maxsize
        wrapper = _LruCacheWrapper(func, 128, False)
        return update_wrapper(wrapper, func)
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
