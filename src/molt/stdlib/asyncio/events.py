"""Public API surface shim for ``asyncio.events``."""

from __future__ import annotations

import contextvars
import os
import signal
import socket
import subprocess
import sys
import threading
import asyncio as _asyncio

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from asyncio import (
    AbstractEventLoop,
    AbstractEventLoopPolicy,
    AbstractServer,
    Handle,
    TimerHandle,
    get_child_watcher,
    get_event_loop,
    get_event_loop_policy,
    get_running_loop,
    new_event_loop,
    set_child_watcher,
    set_event_loop,
    set_event_loop_policy,
)
from asyncio import format_helpers

BaseDefaultEventLoopPolicy = getattr(
    _asyncio, "BaseDefaultEventLoopPolicy", _asyncio.DefaultEventLoopPolicy
)


def on_fork() -> None:
    return None


__all__ = [
    "AbstractEventLoop",
    "AbstractEventLoopPolicy",
    "AbstractServer",
    "BaseDefaultEventLoopPolicy",
    "Handle",
    "TimerHandle",
    "contextvars",
    "format_helpers",
    "get_child_watcher",
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
