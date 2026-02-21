"""Public API surface shim for ``asyncio.base_tasks``."""

from __future__ import annotations

import linecache
import reprlib
import traceback

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

import asyncio.base_futures as base_futures
import asyncio.coroutines as coroutines

__all__ = ["base_futures", "coroutines", "linecache", "reprlib", "traceback"]
