"""Public API surface shim for ``asyncio.timeouts``."""

from __future__ import annotations

from types import TracebackType
import enum
import typing

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

import asyncio.events as events
import asyncio.exceptions as exceptions
import asyncio.tasks as tasks
from asyncio import _Timeout as Timeout
from asyncio import timeout, timeout_at

Optional = getattr(typing, "Optional", None)
if Optional is None or not callable(Optional):

    class _SpecialForm:
        def __call__(self, *args, **kwargs):
            return None

    Optional = _SpecialForm()

Type = getattr(typing, "Type", None)
if Type is None:

    class _SpecialGenericAlias:
        def __call__(self, *args, **kwargs):
            return None

    Type = _SpecialGenericAlias()

final = getattr(typing, "final", None)
if final is None:

    def final(arg):
        return arg


__all__ = [
    "Optional",
    "Timeout",
    "TracebackType",
    "Type",
    "enum",
    "events",
    "exceptions",
    "final",
    "tasks",
    "timeout",
    "timeout_at",
]
