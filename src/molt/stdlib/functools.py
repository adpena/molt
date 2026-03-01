"""Deterministic functools shim for Molt.

Functools helpers are backed by runtime intrinsics; missing intrinsics are a hard error.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


from typing import Any, Callable, Iterable, TYPE_CHECKING


if TYPE_CHECKING:
    pass

__all__ = [
    "cache",
    "cached_property",
    "cmp_to_key",
    "lru_cache",
    "partial",
    "partialmethod",
    "reduce",
    "singledispatch",
    "singledispatchmethod",
    "total_ordering",
    "update_wrapper",
    "wraps",
]

WRAPPER_ASSIGNMENTS = (
    "__module__",
    "__name__",
    "__qualname__",
    "__doc__",
    "__annotations__",
)
WRAPPER_UPDATES = ("__dict__",)


_MOLT_KWD_MARK = _require_intrinsic("molt_functools_kwd_mark", globals())
_MOLT_UPDATE_WRAPPER = _require_intrinsic("molt_functools_update_wrapper", globals())
_MOLT_WRAPS = _require_intrinsic("molt_functools_wraps", globals())
_MOLT_CMP_TO_KEY = _require_intrinsic("molt_functools_cmp_to_key", globals())
_MOLT_TOTAL_ORDERING = _require_intrinsic("molt_functools_total_ordering", globals())
_MOLT_PARTIAL = _require_intrinsic("molt_functools_partial", globals())
_MOLT_REDUCE = _require_intrinsic("molt_functools_reduce", globals())
_MOLT_LRU_CACHE = _require_intrinsic("molt_functools_lru_cache", globals())
_MOLT_SD_NEW = _require_intrinsic("molt_functools_singledispatch_new", globals())
_MOLT_SD_REGISTER = _require_intrinsic(
    "molt_functools_singledispatch_register", globals()
)
_MOLT_SD_CALL = _require_intrinsic("molt_functools_singledispatch_call", globals())
_MOLT_SD_DISPATCH = _require_intrinsic(
    "molt_functools_singledispatch_dispatch", globals()
)
_MOLT_SD_REGISTRY = _require_intrinsic(
    "molt_functools_singledispatch_registry", globals()
)
_MOLT_SD_DROP = _require_intrinsic("molt_functools_singledispatch_drop", globals())

_MISSING = _MOLT_KWD_MARK()


def update_wrapper(
    wrapper: Callable[..., Any],
    wrapped: Callable[..., Any],
    assigned: Iterable[str] = WRAPPER_ASSIGNMENTS,
    updated: Iterable[str] = WRAPPER_UPDATES,
) -> Callable[..., Any]:
    return _MOLT_UPDATE_WRAPPER(wrapper, wrapped, assigned, updated)


def wraps(
    wrapped: Callable[..., Any],
    assigned: Iterable[str] = WRAPPER_ASSIGNMENTS,
    updated: Iterable[str] = WRAPPER_UPDATES,
) -> Callable[[Callable[..., Any]], Callable[..., Any]]:
    return _MOLT_WRAPS(wrapped, assigned, updated)


def cmp_to_key(mycmp: Callable[[Any, Any], Any]) -> Any:
    return _MOLT_CMP_TO_KEY(mycmp)


def total_ordering(cls: type[Any]) -> type[Any]:
    return _MOLT_TOTAL_ORDERING(cls)


def partial(func: Callable[..., Any], /, *args: Any, **kwargs: Any) -> Any:
    return _MOLT_PARTIAL(func, args, kwargs)


def reduce(
    function: Callable[[Any, Any], Any],
    iterable: Iterable[Any],
    initializer: Any = _MISSING,
) -> Any:
    return _MOLT_REDUCE(function, iterable, initializer)


def lru_cache(maxsize: Any = 128, typed: bool = False):
    return _MOLT_LRU_CACHE(maxsize, typed)


def cache(user_function: Callable[..., Any], /) -> Callable[..., Any]:
    """Simple lightweight unbounded cache.  Sometimes called 'memoize'."""
    return lru_cache(maxsize=None)(user_function)


class cached_property:
    """Descriptor that caches the result of a method call as an instance attribute.

    Equivalent to CPython 3.12+ functools.cached_property.  Thread-safe: the
    underlying property function may run more than once on concurrent first
    access, but subsequent accesses return the cached value.
    """

    def __init__(self, func: Callable[..., Any]) -> None:
        self.func = func
        self.attrname: str | None = None
        self.__doc__ = func.__doc__
        self.__module__ = getattr(func, "__module__", None)

    def __set_name__(self, owner: type, name: str) -> None:
        if self.attrname is None:
            self.attrname = name
        elif name != self.attrname:
            raise TypeError(
                "Cannot assign the same cached_property to two different names "
                f"({self.attrname!r} and {name!r})."
            )

    def __get__(self, instance: Any, owner: type | None = None) -> Any:
        if instance is None:
            return self
        if self.attrname is None:
            raise TypeError(
                "Cannot use cached_property instance without calling __set_name__ on it."
            )
        try:
            val = instance.__dict__[self.attrname]
        except KeyError:
            val = self.func(instance)
            instance.__dict__[self.attrname] = val
        return val

    def __class_getitem__(cls, item: Any) -> Any:
        return cls


# ---------------------------------------------------------------------------
# singledispatch
# ---------------------------------------------------------------------------


class singledispatch:
    """Single-dispatch generic function decorator.

    Core dispatch logic is backed by Rust intrinsics; the Python class
    provides the CPython-compatible decorator/register API surface.
    """

    __slots__ = ("_handle", "__wrapped__")

    def __init__(self, func: Callable[..., Any]) -> None:
        self._handle = _MOLT_SD_NEW(func)
        self.__wrapped__ = func

    def __call__(self, *args: Any, **kwargs: Any) -> Any:
        func = _MOLT_SD_CALL(self._handle, args, kwargs)
        return func(*args, **kwargs)

    def register(self, cls: Any = None, func: Any = None) -> Any:
        """Register an implementation for *cls*.

        Supports three calling conventions:
        - ``@f.register(int)`` — explicit type, returns decorator
        - ``@f.register`` — type inferred from first param annotation
        - ``f.register(int, handler)`` — direct registration
        """
        if func is not None:
            # Direct call: f.register(int, handler)
            _MOLT_SD_REGISTER(self._handle, cls, func)
            return func
        if cls is None:
            # Called as decorator factory with no args — shouldn't normally happen
            # but handle gracefully.
            def _decorator(fn: Callable[..., Any]) -> Callable[..., Any]:
                tp = _sd_extract_type(fn)
                _MOLT_SD_REGISTER(self._handle, tp, fn)
                return fn

            return _decorator
        if callable(cls) and not isinstance(cls, type):
            # @f.register applied directly — cls is actually the function
            fn = cls
            tp = _sd_extract_type(fn)
            _MOLT_SD_REGISTER(self._handle, tp, fn)
            return fn

        # @f.register(int) — cls is the type, return decorator
        def _decorator(fn: Callable[..., Any]) -> Callable[..., Any]:
            _MOLT_SD_REGISTER(self._handle, cls, fn)
            return fn

        return _decorator

    def dispatch(self, cls: type) -> Callable[..., Any]:
        """Look up the implementation for *cls*."""
        return _MOLT_SD_DISPATCH(self._handle, cls)

    @property
    def registry(self) -> dict[type, Callable[..., Any]]:
        """Read-only mapping of registered types to implementations."""
        return _MOLT_SD_REGISTRY(self._handle)

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _MOLT_SD_DROP(handle)
            except Exception:
                pass


def _sd_extract_type(func: Callable[..., Any]) -> type:
    """Extract the dispatch type from the first non-return annotation."""
    ann = getattr(func, "__annotations__", {})
    for key, val in ann.items():
        if key != "return":
            return val
    raise TypeError(
        f"singledispatch: cannot determine type from annotations of {func!r}"
    )


# ---------------------------------------------------------------------------
# singledispatchmethod
# ---------------------------------------------------------------------------


class singledispatchmethod:
    """Descriptor that combines :func:`singledispatch` with method binding.

    Dispatch logic is delegated to Rust-backed :class:`singledispatch`;
    this class provides the descriptor protocol surface for class methods.
    """

    __slots__ = ("_sd", "func")

    def __init__(self, func: Callable[..., Any]) -> None:
        self.func = func
        self._sd = singledispatch(func)

    def register(self, cls: Any = None, func: Any = None) -> Any:
        """Delegate registration to the inner singledispatch."""
        return self._sd.register(cls, func)

    def __get__(self, obj: Any, cls: Any = None) -> Any:
        if obj is None:
            return self
        sd_handle = self._sd._handle

        def _method(*args: Any, **kwargs: Any) -> Any:
            dispatch_func = _MOLT_SD_DISPATCH(sd_handle, type(args[0]))
            return dispatch_func(obj, *args, **kwargs)

        return _method


# ---------------------------------------------------------------------------
# partialmethod
# ---------------------------------------------------------------------------


class partialmethod:
    """Descriptor variant of :func:`partial` for use in class bodies.

    Uses the Rust-backed :func:`partial` intrinsic for the actual partial
    application; this class provides the descriptor ``__get__`` protocol.
    """

    __slots__ = ("func", "args", "keywords")

    def __init__(
        self, func: Callable[..., Any], /, *args: Any, **keywords: Any
    ) -> None:
        self.func = func
        self.args = args
        self.keywords = keywords

    def __get__(self, obj: Any, cls: Any = None) -> Any:
        if obj is None:
            return self
        func = self.func
        p_args = self.args
        p_kw = self.keywords
        if p_kw:

            def _bound(*call_args: Any, **call_kw: Any) -> Any:
                return func(obj, *p_args, *call_args, **p_kw, **call_kw)
        else:

            def _bound(*call_args: Any, **call_kw: Any) -> Any:
                return func(obj, *p_args, *call_args, **call_kw)

        return _bound
