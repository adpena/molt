"""Public API surface shim for ``asyncio.events``."""

from __future__ import annotations

import contextvars
import os
import signal
import subprocess
import sys
import threading
import asyncio as _asyncio
from asyncio import socket as socket

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")
_VERSION_INFO = getattr(sys, "version_info", (3, 12, 0, "final", 0))
_EXPOSE_CHILD_WATCHERS = _VERSION_INFO < (3, 14)

from asyncio import (
    AbstractEventLoop,
    AbstractServer,
    Handle,
    TimerHandle,
    get_event_loop,
    get_event_loop_policy,
    get_running_loop,
    new_event_loop,
    set_event_loop,
    set_event_loop_policy,
)
from asyncio import format_helpers

if _VERSION_INFO < (3, 14):
    AbstractEventLoopPolicy = _asyncio.AbstractEventLoopPolicy
    BaseDefaultEventLoopPolicy = getattr(
        _asyncio, "BaseDefaultEventLoopPolicy", _asyncio.DefaultEventLoopPolicy
    )

if _EXPOSE_CHILD_WATCHERS:
    get_child_watcher = _asyncio.get_child_watcher
    set_child_watcher = _asyncio.set_child_watcher


def on_fork() -> None:
    return None


__all__ = [
    "AbstractEventLoop",
    "AbstractServer",
    "Handle",
    "TimerHandle",
    "contextvars",
    "format_helpers",
    "get_event_loop",
    "get_event_loop_policy",
    "get_running_loop",
    "new_event_loop",
    "on_fork",
    "os",
    "set_child_watcher",
    "set_event_loop",
    "set_event_loop_policy",
    "signal",
    "socket",
    "subprocess",
    "sys",
    "threading",
]
if _VERSION_INFO < (3, 14):
    __all__.extend(["AbstractEventLoopPolicy", "BaseDefaultEventLoopPolicy"])
if _EXPOSE_CHILD_WATCHERS:
    __all__.extend(["get_child_watcher", "set_child_watcher"])

globals().pop("_require_intrinsic", None)
