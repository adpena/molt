"""Runtime-native low-level weak-reference authority."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

ReferenceType = _require_intrinsic("molt_weakref_reference_type")()
ref = ReferenceType
_MOLT_WEAKREF_COUNT = _require_intrinsic("molt_weakref_count")
_MOLT_WEAKREF_REFS = _require_intrinsic("molt_weakref_refs")


def getweakrefcount(obj):
    return _MOLT_WEAKREF_COUNT(obj)


def getweakrefs(obj):
    return list(_MOLT_WEAKREF_REFS(obj))

__all__ = [
    "ReferenceType",
    "getweakrefcount",
    "getweakrefs",
    "ref",
]

globals().pop("_require_intrinsic", None)
