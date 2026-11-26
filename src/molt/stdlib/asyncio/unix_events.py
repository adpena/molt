"""Public API surface shim for ``asyncio.unix_events``."""

from __future__ import annotations

import errno
import io
import itertools
import os
import selectors
import signal
import stat
import subprocess
import sys
import threading
import warnings

from . import (
    _SOCKET as socket,
    _VERSION_INFO,
    SelectorEventLoop,
    base_events,
    base_subprocess,
    can_use_pidfd,
    constants,
    coroutines,
    events,
    exceptions,
    futures,
    selector_events,
    tasks,
    transports,
    waitstatus_to_exitcode,
)
from .log import logger

if sys.platform == "win32":
    raise ImportError("Signals are not really supported on Windows")

if _VERSION_INFO >= (3, 13):
    EventLoop = SelectorEventLoop

if _VERSION_INFO < (3, 14):
    from asyncio import (
        AbstractChildWatcher,
        BaseChildWatcher as BaseChildWatcher,
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
