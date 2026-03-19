"""_weakref shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

from weakref import (
    CallableProxyType,
    ProxyType,
    ReferenceType,
    getweakrefcount,
    getweakrefs,
    proxy,
    ref,
)

_MOLT_WEAKREF_COUNT = _require_intrinsic("molt_weakref_count")

__all__ = [
    "CallableProxyType",
    "ProxyType",
    "ReferenceType",
    "getweakrefcount",
    "getweakrefs",
    "proxy",
    "ref",
]

del _MOLT_WEAKREF_COUNT
