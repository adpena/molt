"""Public API surface shim for ``asyncio.unix_events``."""

from __future__ import annotations

import sys as _sys

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

_VERSION_INFO = getattr(_sys, "version_info", (3, 12, 0, "final", 0))

from asyncio import (
    SelectorEventLoop,
    _UnixDefaultEventLoopPolicy as DefaultEventLoopPolicy,
)

if _VERSION_INFO < (3, 14):
    from asyncio import (
        AbstractChildWatcher,
        FastChildWatcher,
        MultiLoopChildWatcher,
        PidfdChildWatcher,
        SafeChildWatcher,
        ThreadedChildWatcher,
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
    __all__ = (
        "SelectorEventLoop",
        "DefaultEventLoopPolicy",
    )

globals().pop("_require_intrinsic", None)
