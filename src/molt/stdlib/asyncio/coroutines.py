"""Public API surface shim for ``asyncio.coroutines``."""

from __future__ import annotations

import collections
import inspect
import os
import sys
import types

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from asyncio import iscoroutine, iscoroutinefunction

__all__ = [
    "collections",
    "inspect",
    "iscoroutine",
    "iscoroutinefunction",
    "os",
    "sys",
    "types",
]
