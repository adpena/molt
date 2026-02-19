"""Public API surface shim for ``asyncio.subprocess``."""

from __future__ import annotations

import subprocess

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

import asyncio.events as events
import asyncio.protocols as protocols
import asyncio.streams as streams
import asyncio.tasks as tasks
from asyncio import (
    Process,
    StreamReaderProtocol as SubprocessStreamProtocol,
    create_subprocess_exec,
    create_subprocess_shell,
)
from asyncio.log import logger

PIPE = -1
STDOUT = -2
DEVNULL = -3

__all__ = [
    "DEVNULL",
    "PIPE",
    "Process",
    "STDOUT",
    "SubprocessStreamProtocol",
    "create_subprocess_exec",
    "create_subprocess_shell",
    "events",
    "logger",
    "protocols",
    "streams",
    "subprocess",
    "tasks",
]
