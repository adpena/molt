"""Public API surface shim for ``asyncio.coroutines``."""

from __future__ import annotations

import collections
import inspect
import os
import sys
import types


from asyncio import iscoroutine, iscoroutinefunction

__all__ = [
    "collections",
    "inspect",
    "iscoroutine",
    "iscoroutinefunction",
    "os",
    "sys",
    "types",
]
