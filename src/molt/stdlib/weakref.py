"""Minimal weakref shim for Molt."""

from __future__ import annotations

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement
# weakref proxy/finalize/WeakKeyDictionary/WeakValueDictionary parity.

from typing import Callable, cast

from _intrinsics import load_intrinsic as _load_intrinsic

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement
# GC-aware weak references + full weakref API once the runtime exposes weakref hooks.

_WEAKREFS: list["ReferenceType"] = []
_MOLT_WEAKREF_REGISTER = _load_intrinsic("molt_weakref_register", globals())
_MOLT_WEAKREF_GET = _load_intrinsic("molt_weakref_get", globals())
_MOLT_WEAKREF_DROP = _load_intrinsic("molt_weakref_drop", globals())
_HAS_INTRINSICS = all(
    func is not None
    for func in (_MOLT_WEAKREF_REGISTER, _MOLT_WEAKREF_GET, _MOLT_WEAKREF_DROP)
)


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


__all__ = ["ReferenceType", "ref"]
