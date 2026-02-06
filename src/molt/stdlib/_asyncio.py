"""Minimal _asyncio shim for Molt."""

from __future__ import annotations

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement
# full _asyncio C-accelerated surface on top of runtime intrinsics.

import sys

_events = sys.modules.get("asyncio.events")
if _events is None:
    import asyncio.events as _events

_get_running_loop = _events._get_running_loop
_set_running_loop = _events._set_running_loop
get_running_loop = _events.get_running_loop
get_event_loop = _events.get_event_loop

__all__ = [
    "_get_running_loop",
    "_set_running_loop",
    "get_running_loop",
    "get_event_loop",
]
