"""Public API surface shim for ``asyncio.mixins``."""

from __future__ import annotations

import threading

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

import asyncio.events as events

__all__ = ["events", "threading"]
