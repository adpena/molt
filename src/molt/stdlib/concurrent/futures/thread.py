"""Public API surface shim for ``concurrent.futures.thread``."""

from __future__ import annotations

import itertools
import os
import queue
import threading
import types
import weakref

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

from concurrent.futures import BrokenThreadPool, ThreadPoolExecutor

__all__ = [
    "BrokenThreadPool",
    "ThreadPoolExecutor",
    "itertools",
    "os",
    "queue",
    "threading",
    "types",
    "weakref",
]

globals().pop("_require_intrinsic", None)
