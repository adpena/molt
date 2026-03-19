"""Public API surface shim for ``asyncio.taskgroups``."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

import asyncio.events as events
import asyncio.exceptions as exceptions
import asyncio.tasks as tasks
from asyncio import TaskGroup

__all__ = ["TaskGroup", "events", "exceptions", "tasks"]

globals().pop("_require_intrinsic", None)
