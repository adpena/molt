"""Public API surface shim for ``asyncio.unix_events``."""

from __future__ import annotations

import sys as _sys
import asyncio as _asyncio

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

_VERSION_INFO = getattr(_sys, "version_info", (3, 12, 0, "final", 0))

from asyncio import SelectorEventLoop

if _VERSION_INFO >= (3, 13):
    EventLoop = _asyncio.EventLoop

if _VERSION_INFO < (3, 14):
    from asyncio import (
        AbstractChildWatcher,
        FastChildWatcher,
        MultiLoopChildWatcher,
        PidfdChildWatcher,
        SafeChildWatcher,
        ThreadedChildWatcher,
        _UnixDefaultEventLoopPolicy as DefaultEventLoopPolicy,
    )

    __all__ = (
        "SelectorEventLoop",
        "AbstractChildWatcher",
        "SafeChildWatcher",
        "FastChildWatcher",
        "PidfdChildWatcher",
        "MultiLoopChildWatcher",
        "ThreadedChildWatcher",
        "DefaultEventLoopPolicy",
    )
else:
    # Child watchers removed in CPython 3.14 (PEP 754).
    __all__ = ["SelectorEventLoop"]
    if _VERSION_INFO >= (3, 13):
        __all__.append("EventLoop")
    __all__ = tuple(__all__)

globals().pop("_require_intrinsic", None)
