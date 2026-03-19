"""Public API surface shim for ``asyncio.futures``."""

from __future__ import annotations

import concurrent
import contextvars
import logging
import sys
import types as _types

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

import asyncio.base_futures as base_futures
import asyncio.events as events
import asyncio.exceptions as exceptions
import asyncio.format_helpers as format_helpers
from asyncio import Future, isfuture, wrap_future

GenericAlias = _types.GenericAlias
STACK_DEBUG = 0

__all__ = [
    "Future",
    "GenericAlias",
    "STACK_DEBUG",
    "base_futures",
    "concurrent",
    "contextvars",
    "events",
    "exceptions",
    "format_helpers",
    "isfuture",
    "logging",
    "sys",
    "wrap_future",
]
