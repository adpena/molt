"""Public API surface shim for ``concurrent.futures.thread``."""

from __future__ import annotations

import itertools
import os
import queue
import threading
import types
import weakref

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

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
