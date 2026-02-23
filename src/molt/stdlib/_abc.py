"""Intrinsic-backed _abc shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


get_cache_token = _require_intrinsic("molt_abc_get_cache_token", globals())
_abc_init = _require_intrinsic("molt_abc_init", globals())
_abc_register = _require_intrinsic("molt_abc_register", globals())
_abc_instancecheck = _require_intrinsic("molt_abc_instancecheck", globals())
_abc_subclasscheck = _require_intrinsic("molt_abc_subclasscheck", globals())
_get_dump = _require_intrinsic("molt_abc_get_dump", globals())
_reset_registry = _require_intrinsic("molt_abc_reset_registry", globals())
_reset_caches = _require_intrinsic("molt_abc_reset_caches", globals())


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
