"""Public API surface shim for ``asyncio.queues``."""

from __future__ import annotations

import collections
import heapq
import types as _types

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

import asyncio.locks as locks
import asyncio.mixins as mixins
from asyncio import LifoQueue, PriorityQueue, Queue, QueueEmpty, QueueFull

GenericAlias = _types.GenericAlias

__all__ = [
    "GenericAlias",
    "LifoQueue",
    "PriorityQueue",
    "Queue",
    "QueueEmpty",
    "QueueFull",
    "collections",
    "heapq",
    "locks",
    "mixins",
]
