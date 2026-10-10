"""Public API surface shim for ``asyncio.staggered``."""

from __future__ import annotations

import contextlib


import asyncio.events as events
import asyncio.exceptions as exceptions_mod
import asyncio.locks as locks
import asyncio.tasks as tasks

# Re-export the canonical implementation from asyncio.__init__.
from asyncio import staggered_race  # noqa: E402


__all__ = [
    "contextlib",
    "events",
    "exceptions_mod",
    "locks",
    "staggered_race",
    "tasks",
]
