"""Public API surface shim for ``asyncio.base_tasks``."""

from __future__ import annotations

import linecache
import reprlib
import traceback
import types as _types

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

try:
    import asyncio.base_futures as base_futures
except Exception:
    base_futures = _types.ModuleType("asyncio.base_futures")

try:
    import asyncio.coroutines as coroutines
except Exception:
    coroutines = _types.ModuleType("asyncio.coroutines")

__all__ = ["base_futures", "coroutines", "linecache", "reprlib", "traceback"]
