"""Minimal _asyncio shim for Molt."""

from __future__ import annotations

# TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement
# full _asyncio C-accelerated surface or native runtime hooks.

import sys

_events = sys.modules.get("asyncio.events")
if _events is None:
    import asyncio.events as _events

_get_running_loop = _events._py__get_running_loop
_set_running_loop = _events._py__set_running_loop
get_running_loop = _events._py_get_running_loop
get_event_loop = _events._py_get_event_loop

__all__ = [
    "_get_running_loop",
    "_set_running_loop",
    "get_running_loop",
    "get_event_loop",
]
