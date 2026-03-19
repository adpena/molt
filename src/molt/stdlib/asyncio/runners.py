"""Public API surface shim for ``asyncio.runners``."""

from __future__ import annotations

import enum
import functools
import signal
import threading
import contextvars

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

import asyncio.constants as constants
import asyncio.coroutines as coroutines
import asyncio.events as events
import asyncio.exceptions as exceptions
import asyncio.tasks as tasks
from asyncio import Runner, run

__all__ = [
    "Runner",
    "constants",
    "contextvars",
    "coroutines",
    "enum",
    "events",
    "exceptions",
    "functools",
    "run",
    "signal",
    "tasks",
    "threading",
]

globals().pop("_require_intrinsic", None)
