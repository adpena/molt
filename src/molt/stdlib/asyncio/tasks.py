"""Public API surface shim for ``asyncio.tasks``."""

from __future__ import annotations

import concurrent
import contextvars
import functools
import inspect
import itertools
import types
import warnings
import weakref

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

import asyncio.base_tasks as base_tasks
import asyncio.coroutines as coroutines
import asyncio.events as events
import asyncio.exceptions as exceptions
import asyncio.futures as futures
import asyncio.timeouts as timeouts
from asyncio import (
    Task,
    all_tasks,
    as_completed,
    create_eager_task_factory,
    create_task,
    current_task,
    eager_task_factory,
    ensure_future,
    gather,
    run_coroutine_threadsafe,
    shield,
    sleep,
    wait,
    wait_for,
)

ALL_COMPLETED = "ALL_COMPLETED"
FIRST_COMPLETED = "FIRST_COMPLETED"
FIRST_EXCEPTION = "FIRST_EXCEPTION"
GenericAlias = types.GenericAlias

__all__ = [
    "ALL_COMPLETED",
    "FIRST_COMPLETED",
    "FIRST_EXCEPTION",
    "GenericAlias",
    "Task",
    "all_tasks",
    "as_completed",
    "base_tasks",
    "concurrent",
    "contextvars",
    "coroutines",
    "create_eager_task_factory",
    "create_task",
    "current_task",
    "eager_task_factory",
    "ensure_future",
    "events",
    "exceptions",
    "functools",
    "futures",
    "gather",
    "inspect",
    "itertools",
    "run_coroutine_threadsafe",
    "shield",
    "sleep",
    "timeouts",
    "types",
    "wait",
    "wait_for",
    "warnings",
    "weakref",
]

globals().pop("_require_intrinsic", None)
