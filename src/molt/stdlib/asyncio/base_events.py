"""Public API surface shim for ``asyncio.base_events``."""

from __future__ import annotations

import collections
import concurrent
import errno
import heapq
import itertools
import os
import socket
import ssl
import stat
import subprocess
import sys
import threading
import time
import traceback
import warnings
import weakref
import logging as _logging

from _intrinsics import require_intrinsic as _require_intrinsic

from asyncio import BaseEventLoop as BaseEventLoop
from asyncio import Server as Server
from . import constants
from . import coroutines
from . import events
from . import exceptions
from . import futures
from . import protocols
from . import sslproto
from . import staggered
from . import tasks
from . import timeouts
from . import transports
from . import trsock

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

MAXIMUM_SELECT_TIMEOUT = 24 * 3600
logger = _logging.getLogger("asyncio")

__all__ = [
    "BaseEventLoop",
    "MAXIMUM_SELECT_TIMEOUT",
    "Server",
    "collections",
    "concurrent",
    "constants",
    "coroutines",
    "errno",
    "events",
    "exceptions",
    "futures",
    "heapq",
    "itertools",
    "logger",
    "os",
    "protocols",
    "socket",
    "ssl",
    "sslproto",
    "staggered",
    "stat",
    "subprocess",
    "sys",
    "tasks",
    "threading",
    "time",
    "timeouts",
    "traceback",
    "transports",
    "trsock",
    "warnings",
    "weakref",
]
