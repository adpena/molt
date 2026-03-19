"""Public API surface shim for ``concurrent.futures.process``."""

from __future__ import annotations

import functools as _functools
import itertools
import multiprocessing
import multiprocessing as mp
import os
import queue
import sys
import threading
from traceback import format_exception
import weakref

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from concurrent.futures import BrokenProcessPool, ProcessPoolExecutor

EXTRA_QUEUED_CALLS = 1


class Queue:
    pass


class partial:
    def __new__(cls, /, *args, **kwargs):
        return _functools.partial(*args, **kwargs)


__all__ = [
    "BrokenProcessPool",
    "EXTRA_QUEUED_CALLS",
    "ProcessPoolExecutor",
    "Queue",
    "format_exception",
    "itertools",
    "mp",
    "multiprocessing",
    "os",
    "partial",
    "queue",
    "sys",
    "threading",
    "weakref",
]
