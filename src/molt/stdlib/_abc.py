"""Intrinsic-backed _abc shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


_bootstrap = _require_intrinsic("molt_abc_bootstrap", globals())
data = _bootstrap()
if not isinstance(data, dict):
    raise RuntimeError("_abc intrinsics unavailable")

get_cache_token = data["get_cache_token"]
_abc_init = data["_abc_init"]
_abc_register = data["_abc_register"]
_abc_instancecheck = data["_abc_instancecheck"]
_abc_subclasscheck = data["_abc_subclasscheck"]
_get_dump = data["_get_dump"]
_reset_registry = data["_reset_registry"]
_reset_caches = data["_reset_caches"]


__all__ = [
    "get_cache_token",
    "_abc_init",
    "_abc_register",
    "_abc_instancecheck",
    "_abc_subclasscheck",
    "_get_dump",
    "_reset_registry",
    "_reset_caches",
]
