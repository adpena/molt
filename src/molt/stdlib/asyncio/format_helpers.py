"""Public API surface shim for ``asyncio.format_helpers``."""

from __future__ import annotations

import functools
import inspect
import reprlib
import sys
import traceback

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

import asyncio.constants as constants


def extract_stack(limit: int | None = None):
    return traceback.extract_stack(limit=limit)


__all__ = [
    "constants",
    "extract_stack",
    "functools",
    "inspect",
    "reprlib",
    "sys",
    "traceback",
]

globals().pop("_require_intrinsic", None)
