"""Public API surface shim for ``asyncio.threads``."""

from __future__ import annotations

import contextvars
import functools

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

import asyncio.events as events
from asyncio import to_thread

__all__ = ["contextvars", "events", "functools", "to_thread"]
