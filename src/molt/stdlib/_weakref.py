"""_weakref shim for Molt."""

from __future__ import annotations

from weakref import (
    CallableProxyType,
    ProxyType,
    ReferenceType,
    getweakrefcount,
    getweakrefs,
    proxy,
    ref,
)

__all__ = [
    "CallableProxyType",
    "ProxyType",
    "ReferenceType",
    "getweakrefcount",
    "getweakrefs",
    "proxy",
    "ref",
]
