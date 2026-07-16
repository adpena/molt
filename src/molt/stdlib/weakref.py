"""weakref shim for Molt with runtime-backed weak references."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
from _weakref import ReferenceType, getweakrefcount, getweakrefs, ref

# Avoid importing typing/abc during weakref bootstrap; these can recurse
# through _weakrefset while this module is still initializing.
Any = object  # type: ignore[assignment]
Callable = Iterable = Iterator = Mapping = object  # type: ignore[assignment]


def cast(_tp, value):  # type: ignore[override]
    return value


_require_intrinsic("molt_stdlib_probe")


def _require_callable_intrinsic(name: str):
    value = _require_intrinsic(name)
    if not callable(value):
        raise RuntimeError(f"{name} intrinsic unavailable")
    return value


_molt_weakref_get = _require_callable_intrinsic("molt_weakref_get")
_molt_weakref_callback = _require_callable_intrinsic("molt_weakref_callback")
_molt_weakref_peek = _require_callable_intrinsic("molt_weakref_peek")
_molt_weakref_finalize_track = _require_callable_intrinsic(
    "molt_weakref_finalize_track"
)
_molt_weakref_finalize_untrack = _require_callable_intrinsic(
    "molt_weakref_finalize_untrack"
)
_molt_weakcontainer_new = _require_callable_intrinsic("molt_weakcontainer_new")
_molt_weakcontainer_store_probe = _require_callable_intrinsic(
    "molt_weakcontainer_store_probe"
)
_molt_weakcontainer_store_commit = _require_callable_intrinsic(
    "molt_weakcontainer_store_commit"
)
_molt_weakcontainer_get = _require_callable_intrinsic("molt_weakcontainer_get")
_molt_weakcontainer_take = _require_callable_intrinsic("molt_weakcontainer_take")
_molt_weakcontainer_contains = _require_callable_intrinsic(
    "molt_weakcontainer_contains"
)
_molt_weakcontainer_len = _require_callable_intrinsic("molt_weakcontainer_len")
_molt_weakcontainer_iter = _require_callable_intrinsic("molt_weakcontainer_iter")
_molt_weakcontainer_refs = _require_callable_intrinsic("molt_weakcontainer_refs")
_molt_weakcontainer_pop = _require_callable_intrinsic("molt_weakcontainer_pop")
_molt_weakcontainer_clear = _require_callable_intrinsic("molt_weakcontainer_clear")
_molt_weakcontainer_dead = _require_callable_intrinsic("molt_weakcontainer_dead")

_WEAK_KEY_DICT = 1
_WEAK_VALUE_DICT = 2
_WEAK_SET = 3
_KEYS = 1
_VALUES = 2
_ITEMS = 3

_MISSING = object()


class KeyedRef(ReferenceType):
    __slots__ = ("key",)

    def __new__(cls, obj, callback, key):
        return ReferenceType.__new__(cls, obj, callback)

    def __init__(
        self,
        obj: object,
        callback: Callable[[ReferenceType], object] | None,
        key: object,
    ) -> None:
        self.key = key


class ProxyType:
    __slots__ = ("_ref",)
    _ref: ReferenceType

    def __init__(self, ref_obj: ReferenceType) -> None:
        object.__setattr__(self, "_ref", ref_obj)

    def _get(self) -> Any:
        obj = self._ref()
        if obj is None:
            raise ReferenceError("weakly-referenced object no longer exists")
        return cast(Any, obj)

    def __getattr__(self, name: str) -> object:
        return getattr(self._get(), name)

    def __setattr__(self, name: str, value: object) -> None:
        setattr(self._get(), name, value)

    def __delattr__(self, name: str) -> None:
        delattr(self._get(), name)

    def __repr__(self) -> str:
        obj = self._ref()
        if obj is None:
            return f"<weakproxy at {hex(id(self))}; dead>"
        return (
            f"<weakproxy at {hex(id(self))}; to '{type(obj).__name__}' "
            f"at {hex(id(obj))}>"
        )

    def __str__(self) -> str:
        return str(self._get())

    def __bytes__(self) -> bytes:
        return bytes(self._get())

    def __format__(self, fmt: str) -> str:
        return format(self._get(), fmt)

    def __bool__(self) -> bool:
        return bool(self._get())

    def __len__(self) -> int:
        return len(self._get())

    def __iter__(self) -> Iterator[object]:
        return iter(self._get())

    def __next__(self) -> object:
        return next(self._get())

    def __getitem__(self, key: object) -> object:
        return self._get()[key]

    def __setitem__(self, key: object, value: object) -> None:
        self._get()[key] = value

    def __delitem__(self, key: object) -> None:
        del self._get()[key]

    def __contains__(self, item: object) -> bool:
        return item in self._get()

    def __hash__(self) -> int:
        return hash(self._get())

    def __eq__(self, other: object) -> bool:
        return self._get() == other

    def __ne__(self, other: object) -> bool:
        return self._get() != other

    def __lt__(self, other: object) -> bool:
        return self._get() < other

    def __le__(self, other: object) -> bool:
        return self._get() <= other

    def __gt__(self, other: object) -> bool:
        return self._get() > other

    def __ge__(self, other: object) -> bool:
        return self._get() >= other

    def __add__(self, other: object) -> object:
        return self._get() + other

    def __radd__(self, other: object) -> object:
        return other + self._get()

    def __sub__(self, other: object) -> object:
        return self._get() - other

    def __rsub__(self, other: object) -> object:
        return other - self._get()

    def __mul__(self, other: object) -> object:
        return self._get() * other

    def __rmul__(self, other: object) -> object:
        return other * self._get()

    def __truediv__(self, other: object) -> object:
        return self._get() / other

    def __rtruediv__(self, other: object) -> object:
        return other / self._get()

    def __floordiv__(self, other: object) -> object:
        return self._get() // other

    def __rfloordiv__(self, other: object) -> object:
        return other // self._get()

    def __mod__(self, other: object) -> object:
        return self._get() % other

    def __rmod__(self, other: object) -> object:
        return other % self._get()

    def __pow__(self, other: object) -> object:
        return self._get() ** other

    def __rpow__(self, other: object) -> object:
        return other ** self._get()

    def __and__(self, other: object) -> object:
        return self._get() & other

    def __rand__(self, other: object) -> object:
        return other & self._get()

    def __or__(self, other: object) -> object:
        return self._get() | other

    def __ror__(self, other: object) -> object:
        return other | self._get()

    def __xor__(self, other: object) -> object:
        return self._get() ^ other

    def __rxor__(self, other: object) -> object:
        return other ^ self._get()

    def __lshift__(self, other: object) -> object:
        return self._get() << other

    def __rlshift__(self, other: object) -> object:
        return other << self._get()

    def __rshift__(self, other: object) -> object:
        return self._get() >> other

    def __rrshift__(self, other: object) -> object:
        return other >> self._get()

    def __neg__(self) -> object:
        return -self._get()

    def __pos__(self) -> object:
        return +self._get()

    def __abs__(self) -> object:
        return abs(self._get())

    def __int__(self) -> int:
        return int(self._get())

    def __float__(self) -> float:
        return float(self._get())

    def __complex__(self) -> complex:
        return complex(self._get())

    def __index__(self) -> int:
        return self._get().__index__()  # type: ignore[attr-defined]

    def __round__(self, ndigits: int | None = None) -> object:
        if ndigits is None:
            return round(self._get())
        return round(self._get(), ndigits)


class CallableProxyType(ProxyType):
    def __call__(self, *args: object, **kwargs: object) -> object:
        return self._get()(*args, **kwargs)


def proxy(
    obj: object, callback: Callable[[ReferenceType], object] | None = None
) -> object:
    ref_obj = ref(obj, callback)
    if callable(obj):
        return CallableProxyType(ref_obj)
    return ProxyType(ref_obj)


class _BoundMethodFallback:
    __slots__ = ("__func__", "__self__")

    def __init__(self, func: Callable[..., object], inst: object) -> None:
        self.__func__ = func
        self.__self__ = inst

    def __call__(self, *args: object, **kwargs: object) -> object:
        return self.__func__(self.__self__, *args, **kwargs)


class WeakMethod:
    __slots__ = ("_self_ref", "_func", "_callback")

    def __init__(self, meth: object, callback: object | None = None) -> None:
        try:
            self_obj = meth.__self__  # type: ignore[attr-defined]
            func = meth.__func__  # type: ignore[attr-defined]
        except Exception as exc:
            raise TypeError("argument should be a bound method") from exc
        self._callback = callback
        self._func = func
        self._self_ref = ref(self_obj, self._handle_dead)

    def _handle_dead(self, _ref: ReferenceType) -> None:
        if self._callback is not None:
            self._callback(self)

    def __call__(self) -> object | None:
        obj = self._self_ref()
        if obj is None:
            return None
        getter = getattr(self._func, "__get__", None)
        if callable(getter):
            return getter(obj, type(obj))
        return _BoundMethodFallback(self._func, obj)

    def __repr__(self) -> str:
        state = "dead" if self() is None else "alive"
        return f"<weakmethod at {hex(id(self))}; {state}>"


class finalize:
    __slots__ = ("_ref", "_func", "_args", "_kwargs", "_alive", "_atexit")

    def __init__(
        self, obj: object, func: Callable[..., Any], /, *args: Any, **kwargs: Any
    ) -> None:
        if not callable(func):
            raise TypeError("finalize() func must be callable")
        self._alive = True
        self._func = func
        self._args = args
        self._kwargs = kwargs
        self._atexit = True
        self._ref = ref(obj, self)
        _molt_weakref_finalize_track(self)

    def __call__(self, _ref: ReferenceType | None = None) -> object | None:
        state = self._take_state()
        if state is None:
            return None
        _, func, args, kwargs = state
        return func(*args, **kwargs)

    def _take_state(
        self,
    ) -> tuple[ReferenceType, object, tuple[object, ...], dict[str, object]] | None:
        if not self._alive:
            return None
        _molt_weakref_finalize_untrack(self)
        self._alive = False
        state = (self._ref, self._func, self._args, self._kwargs)
        self._ref = None  # type: ignore[assignment]
        self._func = None  # type: ignore[assignment]
        self._args = ()
        self._kwargs = {}
        return state

    def detach(
        self,
    ) -> tuple[object, object, tuple[object, ...], dict[str, object]] | None:
        state = self._take_state()
        if state is None:
            return None
        ref_obj, func, args, kwargs = state
        obj = ref_obj()
        if obj is None:
            return None
        return (obj, func, args, kwargs)

    def peek(
        self,
    ) -> tuple[object, object, tuple[object, ...], dict[str, object]] | None:
        if not self._alive:
            return None
        obj = self._ref()
        if obj is None:
            return None
        return (obj, self._func, self._args, self._kwargs)

    @property
    def alive(self) -> bool:
        return self._alive

    @property
    def atexit(self) -> bool:
        return self._atexit

    @atexit.setter
    def atexit(self, value: object) -> None:
        normalized = bool(value)
        self._atexit = normalized

    def __repr__(self) -> str:
        state = "alive" if self._alive else "dead"
        return f"<finalize object at {hex(id(self))}; {state}>"

class WeakKeyDictionary:
    def __init__(self, mapping: dict[object, Any] | None = None) -> None:
        self._state = None

        def remove(weak: ReferenceType, selfref: ReferenceType = ref(self)) -> None:
            owner = selfref()
            if owner is not None and owner._state is not None:
                _molt_weakcontainer_dead(owner._state, weak)

        self._remove = remove
        if mapping is not None:
            self.update(mapping)

    def _ensure_state(self):
        state = self._state
        if state is None:
            state = _molt_weakcontainer_new(_WEAK_KEY_DICT)
            self._state = state
        return state

    def __setitem__(self, key: object, value: Any) -> None:
        key_hash = hash(key)
        state = self._ensure_state()
        if _molt_weakcontainer_store_probe(state, key, value, key_hash):
            return
        key_ref = ReferenceType(key, self._remove)
        key_ref._hash = key_hash
        _molt_weakcontainer_store_commit(
            state, key, value, key_ref, key_hash
        )

    def __getitem__(self, key: object) -> Any:
        key_hash = hash(key)
        if self._state is None:
            raise KeyError(key)
        return _molt_weakcontainer_get(self._state, key, key_hash)

    def __delitem__(self, key: object) -> None:
        key_hash = hash(key)
        if self._state is None:
            raise KeyError(key)
        _molt_weakcontainer_take(self._state, key, key_hash, True)

    def __contains__(self, key: object) -> bool:
        key_hash = hash(key)
        return self._state is not None and bool(
            _molt_weakcontainer_contains(self._state, key, key_hash)
        )

    def __len__(self) -> int:
        if self._state is None:
            return 0
        return int(_molt_weakcontainer_len(self._state))

    def __iter__(self) -> Iterator[object]:
        return self.keys()

    def items(self) -> Iterator[tuple[object, Any]]:
        if self._state is None:
            return iter(())
        return _molt_weakcontainer_iter(self._state, _ITEMS)

    def keys(self) -> Iterator[object]:
        if self._state is None:
            return iter(())
        return _molt_weakcontainer_iter(self._state, _KEYS)

    def values(self) -> Iterator[Any]:
        if self._state is None:
            return iter(())
        return _molt_weakcontainer_iter(self._state, _VALUES)

    def keyrefs(self) -> list[ReferenceType]:
        if self._state is None:
            return []
        return list(_molt_weakcontainer_refs(self._state))

    def get(self, key: object, default: Any = None) -> Any:
        try:
            return self[key]
        except KeyError:
            return default

    def pop(self, key: object, default: Any = _MISSING) -> Any:
        try:
            key_hash = hash(key)
            if self._state is None:
                raise KeyError(key)
            return _molt_weakcontainer_take(self._state, key, key_hash, True)
        except KeyError:
            if default is not _MISSING:
                return default
            raise

    def popitem(self) -> tuple[object, Any]:
        if self._state is None:
            raise KeyError("popitem(): dictionary is empty")
        return _molt_weakcontainer_pop(self._state)

    def setdefault(self, key: object, default: Any = None) -> Any:
        try:
            return self[key]
        except KeyError:
            self[key] = default
            return default

    def update(
        self,
        mapping: Mapping[object, Any] | Iterable[tuple[object, Any]] | None = None,
        **kwargs: Any,
    ) -> None:
        if mapping is not None:
            if hasattr(mapping, "items"):
                for key, value in mapping.items():  # type: ignore[attr-defined]
                    self[key] = value
            else:
                for key, value in mapping:
                    self[key] = value
        for key, value in kwargs.items():
            self[key] = value

    def clear(self) -> None:
        if self._state is not None:
            _molt_weakcontainer_clear(self._state)

    def __repr__(self) -> str:
        return f"<WeakKeyDictionary at {hex(id(self))}>"

    def copy(self) -> "WeakKeyDictionary":
        new_map = WeakKeyDictionary()
        for key, value in self.items():
            new_map[key] = value
        return new_map


class WeakValueDictionary:
    def __init__(self, mapping: dict[object, Any] | None = None) -> None:
        self._state = None

        def remove(weak: ReferenceType, selfref: ReferenceType = ref(self)) -> None:
            owner = selfref()
            if owner is not None and owner._state is not None:
                _molt_weakcontainer_dead(owner._state, weak)

        self._remove = remove
        if mapping is not None:
            self.update(mapping)

    def _ensure_state(self):
        state = self._state
        if state is None:
            state = _molt_weakcontainer_new(_WEAK_VALUE_DICT)
            self._state = state
        return state

    def __setitem__(self, key: object, value: Any) -> None:
        key_hash = hash(key)
        state = self._ensure_state()
        if _molt_weakcontainer_store_probe(state, key, value, key_hash):
            return
        value_ref = KeyedRef(value, self._remove, key)
        _molt_weakcontainer_store_commit(
            state, key, value, value_ref, key_hash
        )

    def __getitem__(self, key: object) -> Any:
        key_hash = hash(key)
        if self._state is None:
            raise KeyError(key)
        return _molt_weakcontainer_get(self._state, key, key_hash)

    def __delitem__(self, key: object) -> None:
        key_hash = hash(key)
        if self._state is None:
            raise KeyError(key)
        _molt_weakcontainer_take(self._state, key, key_hash, True)

    def __contains__(self, key: object) -> bool:
        key_hash = hash(key)
        return self._state is not None and bool(
            _molt_weakcontainer_contains(self._state, key, key_hash)
        )

    def __len__(self) -> int:
        if self._state is None:
            return 0
        return int(_molt_weakcontainer_len(self._state))

    def __iter__(self) -> Iterator[object]:
        return self.keys()

    def items(self) -> Iterator[tuple[object, Any]]:
        if self._state is None:
            return iter(())
        return _molt_weakcontainer_iter(self._state, _ITEMS)

    def keys(self) -> Iterator[object]:
        if self._state is None:
            return iter(())
        return _molt_weakcontainer_iter(self._state, _KEYS)

    def values(self) -> Iterator[Any]:
        if self._state is None:
            return iter(())
        return _molt_weakcontainer_iter(self._state, _VALUES)

    def valuerefs(self) -> list[ReferenceType]:
        if self._state is None:
            return []
        return list(_molt_weakcontainer_refs(self._state))

    def itervaluerefs(self) -> Iterator[ReferenceType]:
        return iter(self.valuerefs())

    def get(self, key: object, default: Any = None) -> Any:
        try:
            return self[key]
        except KeyError:
            return default

    def pop(self, key: object, default: Any = _MISSING) -> Any:
        try:
            key_hash = hash(key)
            if self._state is None:
                raise KeyError(key)
            return _molt_weakcontainer_take(self._state, key, key_hash, True)
        except KeyError:
            if default is not _MISSING:
                return default
            raise

    def update(
        self,
        mapping: Mapping[object, Any] | Iterable[tuple[object, Any]] | None = None,
        **kwargs: Any,
    ) -> None:
        if mapping is not None:
            if hasattr(mapping, "items"):
                for key, value in mapping.items():  # type: ignore[attr-defined]
                    self[key] = value
            else:
                for key, value in mapping:
                    self[key] = value
        for key, value in kwargs.items():
            self[key] = value

    def setdefault(self, key: object, default: Any = None) -> Any:
        try:
            return self[key]
        except KeyError:
            self[key] = default
            return default

    def popitem(self) -> tuple[object, Any]:
        if self._state is None:
            raise KeyError("popitem(): dictionary is empty")
        return _molt_weakcontainer_pop(self._state)

    def clear(self) -> None:
        if self._state is not None:
            _molt_weakcontainer_clear(self._state)

    def __repr__(self) -> str:
        return f"<WeakValueDictionary at {hex(id(self))}>"

    def copy(self) -> "WeakValueDictionary":
        new_map = WeakValueDictionary()
        for key, value in self.items():
            new_map[key] = value
        return new_map


class WeakSet:
    def __init__(self, data: Iterable[object] | None = None) -> None:
        self._state = None

        def remove(weak: ReferenceType, selfref: ReferenceType = ref(self)) -> None:
            owner = selfref()
            if owner is not None and owner._state is not None:
                _molt_weakcontainer_dead(owner._state, weak)

        self._remove = remove
        if data is not None:
            self.update(data)

    def _ensure_state(self):
        state = self._state
        if state is None:
            state = _molt_weakcontainer_new(_WEAK_SET)
            self._state = state
        return state

    def add(self, item: object) -> None:
        item_hash = hash(item)
        state = self._ensure_state()
        if _molt_weakcontainer_store_probe(state, item, item, item_hash):
            return
        item_ref = ReferenceType(item, self._remove)
        item_ref._hash = item_hash
        _molt_weakcontainer_store_commit(
            state, item, item, item_ref, item_hash
        )

    def discard(self, item: object) -> None:
        item_hash = hash(item)
        if self._state is not None:
            _molt_weakcontainer_take(self._state, item, item_hash, False)

    def remove(self, item: object) -> None:
        item_hash = hash(item)
        if self._state is None:
            raise KeyError(item)
        _molt_weakcontainer_take(self._state, item, item_hash, True)

    def pop(self) -> object:
        if self._state is None:
            raise KeyError("pop from empty WeakSet")
        return _molt_weakcontainer_pop(self._state)

    def clear(self) -> None:
        if self._state is not None:
            _molt_weakcontainer_clear(self._state)

    def update(self, data: Iterable[object]) -> None:
        for item in data:
            self.add(item)

    def copy(self) -> "WeakSet":
        return WeakSet(self)

    def difference(self, other: Iterable[object]) -> "WeakSet":
        out = WeakSet()
        for item in self:
            if item not in other:
                out.add(item)
        return out

    def difference_update(self, other: Iterable[object]) -> None:
        for item in list(self):
            if item in other:
                self.discard(item)

    def intersection(self, other: Iterable[object]) -> "WeakSet":
        out = WeakSet()
        for item in self:
            if item in other:
                out.add(item)
        return out

    def intersection_update(self, other: Iterable[object]) -> None:
        for item in list(self):
            if item not in other:
                self.discard(item)

    def symmetric_difference(self, other: Iterable[object]) -> "WeakSet":
        out = WeakSet()
        for item in self:
            if item not in other:
                out.add(item)
        for item in other:
            if item not in self:
                out.add(item)
        return out

    def symmetric_difference_update(self, other: Iterable[object]) -> None:
        for item in list(self):
            if item in other:
                self.discard(item)
        for item in other:
            if item not in self:
                self.add(item)

    def union(self, other: Iterable[object]) -> "WeakSet":
        out = WeakSet()
        out.update(self)
        out.update(other)
        return out

    def isdisjoint(self, other: Iterable[object]) -> bool:
        for item in self:
            if item in other:
                return False
        return True

    def issubset(self, other: Iterable[object]) -> bool:
        for item in self:
            if item not in other:
                return False
        return True

    def issuperset(self, other: Iterable[object]) -> bool:
        for item in other:
            if item not in self:
                return False
        return True

    def __len__(self) -> int:
        if self._state is None:
            return 0
        return int(_molt_weakcontainer_len(self._state))

    def __iter__(self) -> Iterator[object]:
        if self._state is None:
            return iter(())
        return _molt_weakcontainer_iter(self._state, _KEYS)

    def __contains__(self, item: object) -> bool:
        item_hash = hash(item)
        return self._state is not None and bool(
            _molt_weakcontainer_contains(self._state, item, item_hash)
        )

    def __repr__(self) -> str:
        items = list(self)
        if not items:
            return "set()"
        refs = ", ".join(repr(ref(item)) for item in items)
        return f"{{{refs}}}"

    def __or__(self, other: Iterable[object]) -> "WeakSet":
        return self.union(other)

    def __and__(self, other: Iterable[object]) -> "WeakSet":
        return self.intersection(other)

    def __sub__(self, other: Iterable[object]) -> "WeakSet":
        return self.difference(other)

    def __xor__(self, other: Iterable[object]) -> "WeakSet":
        return self.symmetric_difference(other)


__all__ = [
    "CallableProxyType",
    "ProxyType",
    "ReferenceType",
    "WeakKeyDictionary",
    "WeakMethod",
    "WeakSet",
    "WeakValueDictionary",
    "finalize",
    "getweakrefcount",
    "getweakrefs",
    "proxy",
    "ref",
]
