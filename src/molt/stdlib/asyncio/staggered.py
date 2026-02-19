"""Public API surface shim for ``asyncio.staggered``."""

from __future__ import annotations

import contextlib

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

import asyncio.events as events
import asyncio.exceptions as exceptions_mod
import asyncio.locks as locks
import asyncio.tasks as tasks


def staggered_race(*args, **kwargs):
    return args, kwargs


__all__ = [
    "contextlib",
    "events",
    "exceptions_mod",
    "locks",
    "staggered_race",
    "tasks",
]
