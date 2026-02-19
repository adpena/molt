"""Public API surface shim for ``asyncio.__main__``."""

from __future__ import annotations

import ast
import asyncio
import code
import concurrent
import contextvars
import inspect
import sys
import threading
import types
import warnings

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

import asyncio.futures as futures


class AsyncIOInteractiveConsole(code.InteractiveConsole):
    pass


class REPLThread(threading.Thread):
    pass


__all__ = [
    "AsyncIOInteractiveConsole",
    "REPLThread",
    "ast",
    "asyncio",
    "code",
    "concurrent",
    "contextvars",
    "futures",
    "inspect",
    "sys",
    "threading",
    "types",
    "warnings",
]
