"""Public API surface shim for ``asyncio.locks``."""

from __future__ import annotations

import collections
import enum

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

import asyncio.exceptions as exceptions
import asyncio.mixins as mixins
from asyncio import Barrier, BoundedSemaphore, Condition, Event, Lock, Semaphore

__all__ = [
    "Barrier",
    "BoundedSemaphore",
    "Condition",
    "Event",
    "Lock",
    "Semaphore",
    "collections",
    "enum",
    "exceptions",
    "mixins",
]

globals().pop("_require_intrinsic", None)
