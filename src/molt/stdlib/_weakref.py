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

_require_intrinsic("molt_weakref_count", globals())

__all__ = [
    "CallableProxyType",
    "ProxyType",
    "ReferenceType",
    "getweakrefcount",
    "getweakrefs",
    "proxy",
    "ref",
]
