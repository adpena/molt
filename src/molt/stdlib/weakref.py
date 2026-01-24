"""Minimal weakref shim for Molt."""

from __future__ import annotations

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P3, status:partial): implement
# GC-aware weak references + full weakref API once the runtime exposes weakref hooks.

_WEAKREFS: list["ReferenceType"] = []


class ReferenceType:
    __slots__ = ("_obj", "_callback")

    def __init__(self, obj: object, callback: object | None = None) -> None:
        self._obj = obj
        self._callback = callback
        _WEAKREFS.append(self)

    def __call__(self) -> object | None:
        return self._obj

    def __repr__(self) -> str:
        state = "dead" if self._obj is None else "alive"
        return f"<weakref at {hex(id(self))}; {state}>"


def ref(obj: object, callback: object | None = None) -> ReferenceType:
    return ReferenceType(obj, callback)


def _gc_collect_hook() -> None:
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
