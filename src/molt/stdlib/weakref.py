"""weakref shim for Molt with runtime-backed weak references."""

from __future__ import annotations

import builtins as _builtins

from _intrinsics import require_intrinsic as _require_intrinsic


_require_intrinsic("molt_stdlib_probe", globals())


class _TypingAlias:
    __slots__ = ()

    def __getitem__(self, _item):
        return self


Any = object()  # type: ignore[assignment]
Callable = _TypingAlias()  # type: ignore[assignment]
Iterable = _TypingAlias()  # type: ignore[assignment]
Iterator = _TypingAlias()  # type: ignore[assignment]
Mapping = _TypingAlias()  # type: ignore[assignment]
MutableMapping = _TypingAlias()  # type: ignore[assignment]
Sequence = _TypingAlias()  # type: ignore[assignment]


def cast(_tp, value):  # type: ignore[override]
    return value


_molt_weakref_register = getattr(_builtins, "molt_weakref_register", None)
_molt_weakref_get = getattr(_builtins, "molt_weakref_get", None)
_molt_weakref_drop = getattr(_builtins, "molt_weakref_drop", None)
_HAS_INTRINSICS = (
    callable(_molt_weakref_register)
    and callable(_molt_weakref_get)
    and callable(_molt_weakref_drop)
)

_WEAKREFS: list["ReferenceType"] = []
_WEAKREF_REGISTRY: dict[int, list["ReferenceType"]] = {}


def _registry_add(obj: object, ref_obj: "ReferenceType") -> None:
    key = id(obj)
    refs = _WEAKREF_REGISTRY.setdefault(key, [])
    refs.append(ref_obj)
    ref_obj._key = key


def _registry_prune(obj: object) -> list["ReferenceType"]:
    key = id(obj)
    refs = _WEAKREF_REGISTRY.get(key, [])
    kept: list[ReferenceType] = []
    for ref_obj in refs:
        if ref_obj() is obj:
            kept.append(ref_obj)
    if kept:
        _WEAKREF_REGISTRY[key] = kept
    elif key in _WEAKREF_REGISTRY:
        del _WEAKREF_REGISTRY[key]
    return kept


def _registry_remove(ref_obj: "ReferenceType") -> None:
    key = ref_obj._key
    if key is None:
        return
    refs = _WEAKREF_REGISTRY.get(key, [])
    if not refs:
        return
    refs = [entry for entry in refs if entry is not ref_obj]
    if refs:
        _WEAKREF_REGISTRY[key] = refs
    else:
        _WEAKREF_REGISTRY.pop(key, None)


class ReferenceType:
    __slots__ = ("_obj", "_callback", "_key", "_hash", "_tracked", "_registered")

    def __init__(
        self,
        obj: object,
        callback: object | None = None,
        *,
        track: bool = True,
        register: bool = True,
    ) -> None:
        if callback is not None and not callable(callback):
            raise TypeError("weakref callback must be callable")
        self._key: int | None = None
        self._callback = callback
        self._hash: int | None = None
        self._tracked = track
        self._registered = register and _HAS_INTRINSICS
        if self._registered:
            self._obj = None
            _molt_weakref_register(self, obj, callback)  # type: ignore[misc]
        else:
            self._obj = obj
            if register:
                _WEAKREFS.append(self)
        if self._tracked:
            _registry_add(obj, self)

    def __call__(self) -> object | None:
        if self._registered:
            return _molt_weakref_get(self)  # type: ignore[misc]
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
        obj = self()
        if obj is None:
            raise TypeError("weak object has gone away")
        hashed = hash(obj)
        self._hash = hashed
        return hashed

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, ReferenceType):
            return False
        self_obj = self()
        other_obj = other()
        if self_obj is None or other_obj is None:
            return self_obj is other_obj and self._key == other._key
        return self_obj == other_obj

    def __del__(self) -> None:
        if self._tracked:
            _registry_remove(self)
        if self._registered:
            _molt_weakref_drop(self)  # type: ignore[misc]


class KeyedRef(ReferenceType):
    __slots__ = ()

    def __init__(
        self,
        obj: object,
        callback: object | None,
        key_hash: int | None = None,
        *,
        track: bool = True,
        register: bool = True,
    ) -> None:
        super().__init__(obj, callback, track=track, register=register)
        if key_hash is None:
            key_hash = hash(obj)
        self._hash = key_hash


def ref(obj: object, callback: object | None = None) -> ReferenceType:
    if callback is None:
        for ref_obj in _registry_prune(obj):
            if ref_obj._callback is None:
                return ref_obj
    return ReferenceType(obj, callback)


def getweakrefcount(obj: object) -> int:
    return len(_registry_prune(obj))


def getweakrefs(obj: object) -> list[ReferenceType]:
    return list(_registry_prune(obj))


def _gc_collect_hook() -> None:
    if _HAS_INTRINSICS:
        return
    for entry in list(_WEAKREFS):
        if entry._obj is None:
            continue
        entry._obj = None
        callback = entry._callback
        if callback is not None:
            try:
                callback(entry)
            except Exception:
                pass


class ProxyType:
    __slots__ = ("_ref",)

    def __init__(self, ref_obj: ReferenceType) -> None:
        object.__setattr__(self, "_ref", ref_obj)

    def _get(self) -> object:
        obj = self._ref()
        if obj is None:
            raise ReferenceError("weakly-referenced object no longer exists")
        return obj

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


def proxy(obj: object, callback: object | None = None) -> object:
    ref_obj = ref(obj, callback)
    if callable(obj):
        return CallableProxyType(ref_obj)
    return ProxyType(ref_obj)


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
            try:
                self._callback(self)
            except Exception:
                pass

    def __call__(self) -> object | None:
        obj = self._self_ref()
        if obj is None:
            return None
        return self._func.__get__(obj, type(obj))

    def __repr__(self) -> str:
        state = "dead" if self() is None else "alive"
        return f"<weakmethod at {hex(id(self))}; {state}>"


class finalize:
    # TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): wire
    # finalize registry into interpreter shutdown (atexit) once atexit hooks exist.
    atexit = True
    __slots__ = ("_ref", "_func", "_args", "_kwargs", "_alive")

    def __init__(
        self, obj: object, func: object, /, *args: object, **kwargs: object
    ) -> None:
        if not callable(func):
            raise TypeError("finalize() func must be callable")
        self._alive = True
        self._func = func
        self._args = args
        self._kwargs = kwargs
        self._ref = ref(obj, self)

    def __call__(self, _ref: ReferenceType | None = None) -> object | None:
        if not self._alive:
            return None
        self._alive = False
        return self._func(*self._args, **self._kwargs)

    def detach(
        self,
    ) -> tuple[object, object, tuple[object, ...], dict[str, object]] | None:
        if not self._alive:
            return None
        obj = self._ref()
        if obj is None:
            self._alive = False
            return None
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


class WeakKeyDictionary:
    def __init__(self, mapping: dict[object, Any] | None = None) -> None:
        self._data: dict[KeyedRef, Any] = {}
        if mapping is not None:
            self.update(mapping)

    def _remove(self, ref_obj: ReferenceType) -> None:
        self._data.pop(ref_obj, None)

    def _purge(self) -> None:
        for ref_obj in list(self._data.keys()):
            if ref_obj() is None:
                self._data.pop(ref_obj, None)

    def __setitem__(self, key: object, value: Any) -> None:
        ref_obj = KeyedRef(key, self._remove, hash(key))
        self._data[ref_obj] = value

    def __getitem__(self, key: object) -> Any:
        ref_obj = KeyedRef(key, None, hash(key), track=False, register=False)
        value = self._data[ref_obj]
        if ref_obj() is None:
            raise KeyError(key)
        return value

    def __delitem__(self, key: object) -> None:
        ref_obj = KeyedRef(key, None, hash(key), track=False, register=False)
        if ref_obj() is None:
            raise KeyError(key)
        del self._data[ref_obj]

    def __contains__(self, key: object) -> bool:
        try:
            _ = self[key]
            return True
        except KeyError:
            return False

    def __len__(self) -> int:
        self._purge()
        return len(self._data)

    def __iter__(self) -> Iterator[object]:
        items: list[object] = []
        for ref_obj in list(self._data.keys()):
            obj = ref_obj()
            if obj is None:
                self._data.pop(ref_obj, None)
                continue
            items.append(obj)
        return iter(items)

    def items(self) -> Iterator[tuple[object, Any]]:
        items: list[tuple[object, Any]] = []
        for ref_obj, value in list(self._data.items()):
            obj = ref_obj()
            if obj is None:
                self._data.pop(ref_obj, None)
                continue
            items.append((obj, value))
        return iter(items)

    def keys(self) -> Iterator[object]:
        return iter([key for key, _ in self.items()])

    def values(self) -> Iterator[Any]:
        return iter([value for _, value in self.items()])

    def keyrefs(self) -> list[ReferenceType]:
        self._purge()
        return list(self._data.keys())

    def get(self, key: object, default: Any = None) -> Any:
        try:
            return self[key]
        except KeyError:
            return default

    def pop(self, key: object, default: Any = None) -> Any:
        try:
            value = self[key]
        except KeyError:
            if default is not None:
                return default
            raise
        del self[key]
        return value

    def popitem(self) -> tuple[object, Any]:
        self._purge()
        for ref_obj, value in list(self._data.items()):
            obj = ref_obj()
            if obj is None:
                self._data.pop(ref_obj, None)
                continue
            self._data.pop(ref_obj, None)
            return (obj, value)
        raise KeyError("popitem(): dictionary is empty")

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
        self._data.clear()

    def __repr__(self) -> str:
        return f"<WeakKeyDictionary at {hex(id(self))}>"

    def copy(self) -> "WeakKeyDictionary":
        new_map = WeakKeyDictionary()
        for key, value in list(self.items()):
            new_map[key] = value
        return new_map


class WeakValueDictionary:
    def __init__(self, mapping: dict[object, Any] | None = None) -> None:
        self._data: dict[object, ReferenceType] = {}
        if mapping is not None:
            self.update(mapping)

    def _remove(self, key: object) -> Callable[[ReferenceType], None]:
        def _drop(_ref: ReferenceType) -> None:
            self._data.pop(key, None)

        return _drop

    def _purge(self) -> None:
        for key, ref_obj in list(self._data.items()):
            if ref_obj() is None:
                self._data.pop(key, None)

    def __setitem__(self, key: object, value: Any) -> None:
        self._data[key] = ref(value, self._remove(key))

    def __getitem__(self, key: object) -> Any:
        ref_obj = self._data[key]
        obj = ref_obj()
        if obj is None:
            self._data.pop(key, None)
            raise KeyError(key)
        return obj

    def __delitem__(self, key: object) -> None:
        del self._data[key]

    def __contains__(self, key: object) -> bool:
        try:
            _ = self[key]
            return True
        except KeyError:
            return False

    def __len__(self) -> int:
        self._purge()
        return len(self._data)

    def __iter__(self) -> Iterator[object]:
        items: list[object] = []
        for key in list(self._data.keys()):
            try:
                _ = self[key]
            except KeyError:
                continue
            items.append(key)
        return iter(items)

    def items(self) -> Iterator[tuple[object, Any]]:
        items: list[tuple[object, Any]] = []
        for key in list(self._data.keys()):
            try:
                val = self[key]
            except KeyError:
                continue
            items.append((key, val))
        return iter(items)

    def keys(self) -> Iterator[object]:
        return iter([key for key, _ in self.items()])

    def values(self) -> Iterator[Any]:
        return iter([val for _, val in self.items()])

    def valuerefs(self) -> list[ReferenceType]:
        self._purge()
        return list(self._data.values())

    def itervaluerefs(self) -> Iterator[ReferenceType]:
        refs: list[ReferenceType] = []
        for key in list(self._data.keys()):
            ref_obj = self._data.get(key)
            if ref_obj is None:
                continue
            if ref_obj() is None:
                self._data.pop(key, None)
                continue
            refs.append(ref_obj)
        return iter(refs)

    def get(self, key: object, default: Any = None) -> Any:
        try:
            return self[key]
        except KeyError:
            return default

    def pop(self, key: object, default: Any = None) -> Any:
        try:
            val = self[key]
        except KeyError:
            if default is not None:
                return default
            raise
        self._data.pop(key, None)
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
        self._purge()
        for key in list(self._data.keys()):
            try:
                value = self[key]
            except KeyError:
                continue
            self._data.pop(key, None)
            return (key, value)
        raise KeyError("popitem(): dictionary is empty")

    def clear(self) -> None:
        self._data.clear()

    def __repr__(self) -> str:
        return f"<WeakValueDictionary at {hex(id(self))}>"

    def copy(self) -> "WeakValueDictionary":
        new_map = WeakValueDictionary()
        for key, value in list(self.items()):
            new_map[key] = value
        return new_map


class WeakSet:
    def __init__(self, data: Iterable[object] | None = None) -> None:
        self._data: dict[ReferenceType, None] = {}
        if data is not None:
            self.update(data)

    def _remove(self, ref_obj: ReferenceType) -> None:
        self._data.pop(ref_obj, None)

    def _purge(self) -> None:
        for ref_obj in list(self._data.keys()):
            if ref_obj() is None:
                self._data.pop(ref_obj, None)

    def add(self, item: object) -> None:
        ref_obj = ref(item, self._remove)
        try:
            self._data[ref_obj] = None
        except TypeError:
            raise TypeError(
                "cannot use 'weakref.ReferenceType' as a set element "
                f"(unhashable type: '{type(item).__name__}')"
            ) from None

    def discard(self, item: object) -> None:
        for ref_obj in list(self._data.keys()):
            if ref_obj() is item:
                self._data.pop(ref_obj, None)
                break

    def remove(self, item: object) -> None:
        for ref_obj in list(self._data.keys()):
            if ref_obj() is item:
                self._data.pop(ref_obj, None)
                return
        raise KeyError(item)

    def pop(self) -> object:
        self._purge()
        for ref_obj in list(self._data.keys()):
            obj = ref_obj()
            if obj is not None:
                self._data.pop(ref_obj, None)
                return obj
        raise KeyError("pop from empty WeakSet")

    def clear(self) -> None:
        self._data.clear()

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
        self._purge()
        return len(self._data)

    def __iter__(self) -> Iterator[object]:
        items: list[object] = []
        for ref_obj in list(self._data.keys()):
            obj = ref_obj()
            if obj is None:
                self._data.pop(ref_obj, None)
                continue
            items.append(obj)
        return iter(items)

    def __contains__(self, item: object) -> bool:
        for ref_obj in list(self._data.keys()):
            obj = ref_obj()
            if obj is None:
                self._data.pop(ref_obj, None)
                continue
            if obj is item:
                return True
        return False

    def __repr__(self) -> str:
        self._purge()
        if not self._data:
            return "set()"
        items = ", ".join(repr(ref_obj) for ref_obj in self._data.keys())
        return f"{{{items}}}"

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
