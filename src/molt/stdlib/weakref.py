"""weakref shim for Molt with runtime-backed weak references."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

# Avoid importing typing/abc during weakref bootstrap; these can recurse
# through _weakrefset while this module is still initializing.
Any = object  # type: ignore[assignment]
Callable = Iterable = Iterator = Mapping = object  # type: ignore[assignment]


def cast(_tp, value):  # type: ignore[override]
    return value


_require_intrinsic("molt_stdlib_probe", globals())


def _require_callable_intrinsic(name: str):
    value = _require_intrinsic(name, globals())
    if not callable(value):
        raise RuntimeError(f"{name} intrinsic unavailable")
    return value


_molt_weakref_register = _require_callable_intrinsic("molt_weakref_register")
_molt_weakref_get = _require_callable_intrinsic("molt_weakref_get")
_molt_weakref_peek = _require_callable_intrinsic("molt_weakref_peek")
_molt_weakref_drop = _require_callable_intrinsic("molt_weakref_drop")
_molt_weakref_collect = _require_callable_intrinsic("molt_weakref_collect")
_molt_weakref_find_nocallback = _require_callable_intrinsic(
    "molt_weakref_find_nocallback"
)
_molt_weakref_refs = _require_callable_intrinsic("molt_weakref_refs")
_molt_weakref_count = _require_callable_intrinsic("molt_weakref_count")
_molt_weakref_finalize_track = _require_callable_intrinsic(
    "molt_weakref_finalize_track"
)
_molt_weakref_finalize_untrack = _require_callable_intrinsic(
    "molt_weakref_finalize_untrack"
)
_molt_weakkeydict_set = _require_callable_intrinsic("molt_weakkeydict_set")
_molt_weakkeydict_get = _require_callable_intrinsic("molt_weakkeydict_get")
_molt_weakkeydict_del = _require_callable_intrinsic("molt_weakkeydict_del")
_molt_weakkeydict_contains = _require_callable_intrinsic("molt_weakkeydict_contains")
_molt_weakkeydict_len = _require_callable_intrinsic("molt_weakkeydict_len")
_molt_weakkeydict_items = _require_callable_intrinsic("molt_weakkeydict_items")
_molt_weakkeydict_keyrefs = _require_callable_intrinsic("molt_weakkeydict_keyrefs")
_molt_weakkeydict_popitem = _require_callable_intrinsic("molt_weakkeydict_popitem")
_molt_weakkeydict_clear = _require_callable_intrinsic("molt_weakkeydict_clear")
_molt_weakvaluedict_set = _require_callable_intrinsic("molt_weakvaluedict_set")
_molt_weakvaluedict_get = _require_callable_intrinsic("molt_weakvaluedict_get")
_molt_weakvaluedict_del = _require_callable_intrinsic("molt_weakvaluedict_del")
_molt_weakvaluedict_contains = _require_callable_intrinsic(
    "molt_weakvaluedict_contains"
)
_molt_weakvaluedict_len = _require_callable_intrinsic("molt_weakvaluedict_len")
_molt_weakvaluedict_items = _require_callable_intrinsic("molt_weakvaluedict_items")
_molt_weakvaluedict_valuerefs = _require_callable_intrinsic(
    "molt_weakvaluedict_valuerefs"
)
_molt_weakvaluedict_popitem = _require_callable_intrinsic("molt_weakvaluedict_popitem")
_molt_weakvaluedict_clear = _require_callable_intrinsic("molt_weakvaluedict_clear")
_molt_weakset_add = _require_callable_intrinsic("molt_weakset_add")
_molt_weakset_discard = _require_callable_intrinsic("molt_weakset_discard")
_molt_weakset_remove = _require_callable_intrinsic("molt_weakset_remove")
_molt_weakset_pop = _require_callable_intrinsic("molt_weakset_pop")
_molt_weakset_contains = _require_callable_intrinsic("molt_weakset_contains")
_molt_weakset_len = _require_callable_intrinsic("molt_weakset_len")
_molt_weakset_items = _require_callable_intrinsic("molt_weakset_items")
_molt_weakset_clear = _require_callable_intrinsic("molt_weakset_clear")

_MISSING = object()


class ReferenceType:
    __slots__ = ("_obj", "_callback", "_key", "_hash", "_registered")

    def __init__(
        self,
        obj: object,
        callback: Callable[["ReferenceType"], object] | None = None,
        *,
        track: bool = True,
        register: bool = True,
    ) -> None:
        del track
        if callback is not None and not callable(callback):
            raise TypeError("weakref callback must be callable")
        self._key = id(obj)
        self._callback = callback
        self._hash: int | None = None
        self._registered = register
        if register:
            self._obj = None
            _molt_weakref_register(self, obj, callback)  # type: ignore[misc]
        else:
            self._obj = obj

    def __call__(self) -> object | None:
        if self._registered:
            return _molt_weakref_get(self)  # type: ignore[misc]
        return self._obj

    def _peek_obj(self) -> object | None:
        if self._registered:
            return _molt_weakref_peek(self)  # type: ignore[misc]
        return self._obj

    def __repr__(self) -> str:
        obj = self()
        if obj is None:
            return f"<weakref at {hex(id(self))}; dead>"
        return (
            f"<weakref at {hex(id(self))}; to '{type(obj).__name__}' at {hex(id(obj))}>"
        )

    def __hash__(self) -> int:
        if self._hash is not None:
            return self._hash
        obj = self._peek_obj()
        if obj is None:
            raise TypeError("weak object has gone away")
        hashed = hash(obj)
        self._hash = hashed
        return hashed

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, ReferenceType):
            return False
        self_obj = self._peek_obj()
        other_obj = other._peek_obj()
        if self_obj is None or other_obj is None:
            return self is other
        return self_obj == other_obj

    def __del__(self) -> None:
        if self._registered:
            _molt_weakref_drop(self)  # type: ignore[misc]


class KeyedRef(ReferenceType):
    __slots__ = ()

    def __init__(
        self,
        obj: object,
        callback: Callable[[ReferenceType], object] | None,
        key_hash: int | None = None,
        *,
        track: bool = True,
        register: bool = True,
    ) -> None:
        super().__init__(obj, callback, track=track, register=register)
        if key_hash is None:
            key_hash = hash(obj)
        self._hash = key_hash


def ref(
    obj: object, callback: Callable[[ReferenceType], object] | None = None
) -> ReferenceType:
    if callback is None:
        cached = _molt_weakref_find_nocallback(obj)
        if isinstance(cached, ReferenceType):
            return cached
    result = ReferenceType(obj, callback)
    # Drop local strong refs eagerly; some runtime paths keep frame locals alive
    # longer than CPython, which can delay weakref invalidation/callbacks.
    obj = None
    callback = None
    return result


def getweakrefcount(obj: object) -> int:
    value = _molt_weakref_count(obj)
    if not isinstance(value, int):
        raise RuntimeError("weakref count intrinsic returned invalid value")
    return int(value)


def getweakrefs(obj: object) -> list[ReferenceType]:
    refs = _molt_weakref_refs(obj)
    if not isinstance(refs, list):
        raise RuntimeError("weakref refs intrinsic returned invalid value")
    if not all(isinstance(entry, ReferenceType) for entry in refs):
        raise RuntimeError("weakref refs intrinsic returned invalid value")
    return list(refs)


def _gc_collect_hook() -> None:
    _molt_weakref_collect()  # type: ignore[misc]


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
        self_obj = None
        meth = None

    def _handle_dead(self, _ref: ReferenceType) -> None:
        if self._callback is not None:
            try:
                self._callback(self)
            except Exception:
                pass

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
    atexit = True
    __slots__ = ("_ref", "_func", "_args", "_kwargs", "_alive", "atexit")

    def __init__(
        self, obj: object, func: Callable[..., Any], /, *args: Any, **kwargs: Any
    ) -> None:
        if not callable(func):
            raise TypeError("finalize() func must be callable")
        self._alive = True
        self._func = func
        self._args = args
        self._kwargs = kwargs
        self.atexit = True
        self._ref = ref(obj, self)
        _molt_weakref_finalize_track(self)
        obj = None

    def __call__(self, _ref: ReferenceType | None = None) -> object | None:
        if not self._alive:
            return None
        _molt_weakref_finalize_untrack(self)
        self._alive = False
        return self._func(*self._args, **self._kwargs)

    def detach(
        self,
    ) -> tuple[object, object, tuple[object, ...], dict[str, object]] | None:
        if not self._alive:
            return None
        obj = self._ref()
        if obj is None:
            _molt_weakref_finalize_untrack(self)
            self._alive = False
            return None
        _molt_weakref_finalize_untrack(self)
        self._alive = False
        return (obj, self._func, self._args, self._kwargs)

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

    def __repr__(self) -> str:
        state = "alive" if self._alive else "dead"
        return f"<finalize object at {hex(id(self))}; {state}>"

    def __del__(self) -> None:
        if self._alive:
            _molt_weakref_finalize_untrack(self)


class WeakKeyDictionary:
    def __init__(self, mapping: dict[object, Any] | None = None) -> None:
        if mapping is not None:
            self.update(mapping)

    def __setitem__(self, key: object, value: Any) -> None:
        key_hash = hash(key)
        key_ref = KeyedRef(key, None, key_hash)
        _molt_weakkeydict_set(self, key, key_ref, key_hash, value)
        key = None

    def __getitem__(self, key: object) -> Any:
        key_hash = hash(key)
        return _molt_weakkeydict_get(self, key, key_hash)

    def __delitem__(self, key: object) -> None:
        key_hash = hash(key)
        _molt_weakkeydict_del(self, key, key_hash)

    def __contains__(self, key: object) -> bool:
        key_hash = hash(key)
        return bool(_molt_weakkeydict_contains(self, key, key_hash))

    def __len__(self) -> int:
        return int(_molt_weakkeydict_len(self))

    def __iter__(self) -> Iterator[object]:
        return iter([key for key, _ in self.items()])

    def items(self) -> Iterator[tuple[object, Any]]:
        entries = _molt_weakkeydict_items(self)
        return iter(list(entries))

    def keys(self) -> Iterator[object]:
        return iter([key for key, _ in self.items()])

    def values(self) -> Iterator[Any]:
        return iter([value for _, value in self.items()])

    def keyrefs(self) -> list[ReferenceType]:
        refs = _molt_weakkeydict_keyrefs(self)
        return list(refs)

    def get(self, key: object, default: Any = None) -> Any:
        try:
            return self[key]
        except KeyError:
            return default

    def pop(self, key: object, default: Any = _MISSING) -> Any:
        try:
            value = self[key]
        except KeyError:
            if default is not _MISSING:
                return default
            raise
        del self[key]
        return value

    def popitem(self) -> tuple[object, Any]:
        return _molt_weakkeydict_popitem(self)

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
        _molt_weakkeydict_clear(self)

    def __repr__(self) -> str:
        return f"<WeakKeyDictionary at {hex(id(self))}>"

    def copy(self) -> "WeakKeyDictionary":
        new_map = WeakKeyDictionary()
        for key, value in list(self.items()):
            new_map[key] = value
        return new_map


class WeakValueDictionary:
    def __init__(self, mapping: dict[object, Any] | None = None) -> None:
        if mapping is not None:
            self.update(mapping)

    def __setitem__(self, key: object, value: Any) -> None:
        key_hash = hash(key)
        value_ref = ReferenceType(value, None)
        _molt_weakvaluedict_set(self, key, key_hash, value_ref)
        value = None

    def __getitem__(self, key: object) -> Any:
        key_hash = hash(key)
        return _molt_weakvaluedict_get(self, key, key_hash)

    def __delitem__(self, key: object) -> None:
        key_hash = hash(key)
        _molt_weakvaluedict_del(self, key, key_hash)

    def __contains__(self, key: object) -> bool:
        key_hash = hash(key)
        return bool(_molt_weakvaluedict_contains(self, key, key_hash))

    def __len__(self) -> int:
        return int(_molt_weakvaluedict_len(self))

    def __iter__(self) -> Iterator[object]:
        return iter([key for key, _ in self.items()])

    def items(self) -> Iterator[tuple[object, Any]]:
        entries = _molt_weakvaluedict_items(self)
        return iter(list(entries))

    def keys(self) -> Iterator[object]:
        return iter([key for key, _ in self.items()])

    def values(self) -> Iterator[Any]:
        return iter([val for _, val in self.items()])

    def valuerefs(self) -> list[ReferenceType]:
        refs = _molt_weakvaluedict_valuerefs(self)
        return list(refs)

    def itervaluerefs(self) -> Iterator[ReferenceType]:
        refs = _molt_weakvaluedict_valuerefs(self)
        return iter(list(refs))

    def get(self, key: object, default: Any = None) -> Any:
        try:
            return self[key]
        except KeyError:
            return default

    def pop(self, key: object, default: Any = _MISSING) -> Any:
        try:
            val = self[key]
        except KeyError:
            if default is not _MISSING:
                return default
            raise
        del self[key]
        return val

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
        return _molt_weakvaluedict_popitem(self)

    def clear(self) -> None:
        _molt_weakvaluedict_clear(self)

    def __repr__(self) -> str:
        return f"<WeakValueDictionary at {hex(id(self))}>"

    def copy(self) -> "WeakValueDictionary":
        new_map = WeakValueDictionary()
        for key, value in list(self.items()):
            new_map[key] = value
        return new_map


class WeakSet:
    def __init__(self, data: Iterable[object] | None = None) -> None:
        if data is not None:
            self.update(data)

    def add(self, item: object) -> None:
        item_hash = hash(item)
        item_ref = ReferenceType(item, None)
        _molt_weakset_add(self, item, item_ref, item_hash)

    def discard(self, item: object) -> None:
        item_hash = hash(item)
        _molt_weakset_discard(self, item, item_hash)

    def remove(self, item: object) -> None:
        item_hash = hash(item)
        _molt_weakset_remove(self, item, item_hash)

    def pop(self) -> object:
        return _molt_weakset_pop(self)

    def clear(self) -> None:
        _molt_weakset_clear(self)

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
        return int(_molt_weakset_len(self))

    def __iter__(self) -> Iterator[object]:
        items = _molt_weakset_items(self)
        return iter(list(items))

    def __contains__(self, item: object) -> bool:
        item_hash = hash(item)
        return bool(_molt_weakset_contains(self, item, item_hash))

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
