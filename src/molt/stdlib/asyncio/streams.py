"""Public API surface shim for ``asyncio.streams``."""

from __future__ import annotations

import collections
import socket
import sys
import warnings
import weakref

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

import asyncio.coroutines as coroutines
import asyncio.events as events
import asyncio.exceptions as exceptions
import asyncio.format_helpers as format_helpers
import asyncio.protocols as protocols
from asyncio import (
    FlowControlMixin,
    StreamReader,
    StreamReaderProtocol,
    StreamWriter,
    open_connection,
    open_unix_connection,
    sleep,
    start_server,
    start_unix_server,
)
from asyncio.log import logger

__all__ = [
    "FlowControlMixin",
    "StreamReader",
    "StreamReaderProtocol",
    "StreamWriter",
    "collections",
    "coroutines",
    "events",
    "exceptions",
    "format_helpers",
    "logger",
    "open_connection",
    "open_unix_connection",
    "protocols",
    "sleep",
    "socket",
    "start_server",
    "start_unix_server",
    "sys",
    "warnings",
    "weakref",
]
