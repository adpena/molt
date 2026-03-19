"""Public API surface shim for ``asyncio.selector_events``."""

from __future__ import annotations

import collections
import errno
import functools
import itertools
import os
import selectors
import socket
import ssl
import weakref
import warnings

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

import asyncio.base_events as base_events
import asyncio.constants as constants
import asyncio.events as events
import asyncio.futures as futures
import asyncio.protocols as protocols
import asyncio.sslproto as sslproto
import asyncio.transports as transports
import asyncio.trsock as trsock
from asyncio import SelectorEventLoop as BaseSelectorEventLoop
from asyncio.log import logger

try:
    SC_IOV_MAX = int(os.sysconf("SC_IOV_MAX"))
except Exception:
    SC_IOV_MAX = 1024

__all__ = [
    "BaseSelectorEventLoop",
    "SC_IOV_MAX",
    "base_events",
    "collections",
    "constants",
    "errno",
    "events",
    "functools",
    "futures",
    "itertools",
    "logger",
    "os",
    "protocols",
    "selectors",
    "socket",
    "ssl",
    "sslproto",
    "transports",
    "trsock",
    "warnings",
    "weakref",
]

globals().pop("_require_intrinsic", None)
