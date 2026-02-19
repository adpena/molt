"""Public API surface shim for ``asyncio.unix_events``."""

from __future__ import annotations


from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

from asyncio import (
    AbstractChildWatcher,
    FastChildWatcher,
    MultiLoopChildWatcher,
    PidfdChildWatcher,
    SafeChildWatcher,
    SelectorEventLoop,
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
