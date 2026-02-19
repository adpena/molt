"""Public API surface shim for ``asyncio.proactor_events``."""

from __future__ import annotations

import collections
import io
import logging as _logging
import os
import signal
import socket
import threading
import warnings

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

import asyncio.base_events as base_events
import asyncio.constants as constants
import asyncio.exceptions as exceptions
import asyncio.futures as futures
import asyncio.protocols as protocols
import asyncio.sslproto as sslproto
import asyncio.transports as transports
import asyncio.trsock as trsock
from asyncio import SelectorEventLoop as BaseProactorEventLoop

logger = _logging.getLogger("asyncio")

__all__ = [
    "BaseProactorEventLoop",
    "base_events",
    "collections",
    "constants",
    "exceptions",
    "futures",
    "io",
    "logger",
    "os",
    "protocols",
    "signal",
    "socket",
    "sslproto",
    "threading",
    "transports",
    "trsock",
    "warnings",
]
