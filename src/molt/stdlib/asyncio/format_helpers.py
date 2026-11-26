"""Public API surface shim for ``asyncio.format_helpers``."""

from __future__ import annotations

import functools
import inspect
import reprlib
import sys
import traceback
from typing import Any


import asyncio.constants as constants


def _format_callback_source(
    callback: Any, args: tuple[Any, ...], *, debug: bool = False
) -> str:
    name = getattr(callback, "__qualname__", None) or getattr(
        callback, "__name__", None
    )
    if name is None:
        name = repr(callback)
    if args:
        args_repr = ", ".join(repr(arg) for arg in args)
        return f"{name}({args_repr})"
    return f"{name}()"

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
