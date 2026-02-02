"""Minimal weakref shim for Molt."""

from __future__ import annotations

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement
# weakref proxy/finalize/WeakValueDictionary parity.

from typing import Callable, Iterable, Any, cast

from _intrinsics import load_intrinsic as _load_intrinsic

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement
# GC-aware weak references + full weakref API once the runtime exposes weakref hooks.

_WEAKREFS: list["ReferenceType"] = []
_MOLT_WEAKREF_REGISTER = _load_intrinsic("molt_weakref_register", globals())
_MOLT_WEAKREF_GET = _load_intrinsic("molt_weakref_get", globals())
_MOLT_WEAKREF_DROP = _load_intrinsic("molt_weakref_drop", globals())
_HAS_INTRINSICS = False
if all(
    func is not None
    for func in (_MOLT_WEAKREF_REGISTER, _MOLT_WEAKREF_GET, _MOLT_WEAKREF_DROP)
):
    # TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): enable
    # runtime weakref intrinsics once GC hooks are wired end-to-end.
    _HAS_INTRINSICS = False


class ReferenceType:
    __slots__ = ("_obj", "_callback")

    def __init__(self, obj: object, callback: object | None = None) -> None:
        if _HAS_INTRINSICS:
            self._obj = None
            self._callback = None
            register = cast(
                Callable[[ReferenceType, object, object | None], object],
                _MOLT_WEAKREF_REGISTER,
            )
            register(self, obj, callback)
        else:
            self._obj = obj
            self._callback = callback
            _WEAKREFS.append(self)

    def __call__(self) -> object | None:
        if _HAS_INTRINSICS:
            get_ref = cast(Callable[[ReferenceType], object], _MOLT_WEAKREF_GET)
            return get_ref(self)
        return self._obj

    def __repr__(self) -> str:
        state = "dead" if self() is None else "alive"
        return f"<weakref at {hex(id(self))}; {state}>"

    def __del__(self) -> None:
        if _HAS_INTRINSICS and _MOLT_WEAKREF_DROP is not None:
            drop_ref = cast(Callable[[ReferenceType], object], _MOLT_WEAKREF_DROP)
            drop_ref(self)


def ref(obj: object, callback: object | None = None) -> ReferenceType:
    return ReferenceType(obj, callback)


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


__all__ = ["ReferenceType", "WeakKeyDictionary", "ref"]


class WeakKeyDictionary:
    def __init__(self, mapping: dict[object, Any] | None = None) -> None:
        self._data: dict[int, tuple[ReferenceType, Any]] = {}
        if mapping is not None:
            for key, value in mapping.items():
                self[key] = value

    def _remove(self, key_id: int) -> Callable[[ReferenceType], None]:
        def _drop(_ref: ReferenceType) -> None:
            self._data.pop(key_id, None)

        return _drop

    def _purge(self) -> None:
        for key_id, (ref_obj, _) in list(self._data.items()):
            if ref_obj() is None:
                self._data.pop(key_id, None)

    def __setitem__(self, key: object, value: Any) -> None:
        ref_obj = ref(key, self._remove(id(key)))
        self._data[id(key)] = (ref_obj, value)

    def __getitem__(self, key: object) -> Any:
        entry = self._data[id(key)]
        if entry[0]() is not key:
            raise KeyError(key)
        return entry[1]

    def __delitem__(self, key: object) -> None:
        entry = self._data[id(key)]
        if entry[0]() is not key:
            raise KeyError(key)
        del self._data[id(key)]

    def __contains__(self, key: object) -> bool:
        entry = self._data.get(id(key))
        if entry is None:
            return False
        if entry[0]() is not key:
            return False
        return True

    def __len__(self) -> int:
        self._purge()
        return len(self._data)

    def __iter__(self) -> Iterable[object]:
        for key_id, (ref_obj, _) in list(self._data.items()):
            obj = ref_obj()
            if obj is None:
                self._data.pop(key_id, None)
                continue
            yield obj

    def items(self) -> list[tuple[object, Any]]:
        items: list[tuple[object, Any]] = []
        for key_id, (ref_obj, value) in list(self._data.items()):
            obj = ref_obj()
            if obj is None:
                self._data.pop(key_id, None)
                continue
            items.append((obj, value))
        return items

    def keys(self) -> list[object]:
        return [key for key, _ in self.items()]

    def values(self) -> list[Any]:
        return [value for _, value in self.items()]

    def get(self, key: object, default: Any = None) -> Any:
        try:
            return self[key]
        except KeyError:
            return default
