"""CPython-shaped multiprocessing facade backed by Molt intrinsic core."""

from __future__ import annotations

import builtins as _builtins
import os as _os
import queue as _queue
import sys as _sys
import threading as _threading
import types as _types

from _intrinsics import require_intrinsic as _require_intrinsic

import multiprocessing._core as _core
from multiprocessing._api_surface import apply_module_api_surface as _apply_api_surface


_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")


class ProcessError(Exception):
    pass


class BufferTooShort(ProcessError):
    pass


class AuthenticationError(ProcessError):
    pass


class _CompatProcess:
    __slots__ = ("name", "pid")

    def __init__(self) -> None:
        self.name = "MainProcess"
        self.pid = _os.getpid()


_MAIN_PROCESS = _CompatProcess()


class _CompatLogger:
    __slots__ = ("level",)

    def __init__(self) -> None:
        self.level = 0

    def setLevel(self, level) -> None:
        try:
            self.level = int(level)
        except Exception:
            self.level = 0

    def debug(self, *_args, **_kwargs) -> None:
        return None

    def info(self, *_args, **_kwargs) -> None:
        return None

    def warning(self, *_args, **_kwargs) -> None:
        return None

    def error(self, *_args, **_kwargs) -> None:
        return None


_LOGGER = _CompatLogger()


def active_children():
    return []


def current_process():
    return _MAIN_PROCESS


def parent_process():
    return None


class _TopLevelFacade:
    def Array(self, typecode: str, values):
        return _core.Array(typecode, values)

    def RawArray(self, typecode: str, values):
        return _core.Array(typecode, values)

    def RawValue(self, typecode: str, value):
        return _core.Value(typecode, value)

    def Value(self, typecode: str, value):
        return _core.Value(typecode, value)

    def Barrier(self, parties: int, action=None, timeout=None):
        return _threading.Barrier(parties, action=action, timeout=timeout)

    def BoundedSemaphore(self, value: int = 1):
        return _threading.BoundedSemaphore(value)

    def Condition(self, lock=None):
        return _threading.Condition(lock)

    def Event(self):
        return _threading.Event()

    def JoinableQueue(self, maxsize: int = 0):
        return _queue.Queue(maxsize=maxsize)

    def Lock(self):
        return _threading.Lock()

    def Manager(self):
        raise RuntimeError("multiprocessing.Manager is not implemented in Molt yet")

    def Pipe(self, duplex: bool = True):
        return _core.Pipe(duplex=duplex)

    def Pool(self, *args, **kwargs):
        return _core.Pool(*args, **kwargs)

    def Queue(self, maxsize: int = 0):
        return _core.Queue(maxsize)

    def RLock(self):
        return _threading.RLock()

    def Semaphore(self, value: int = 1):
        return _threading.Semaphore(value)

    def SimpleQueue(self):
        return _queue.SimpleQueue()

    def allow_connection_pickling(self):
        return None

    def cpu_count(self):
        return _os.cpu_count() or 1

    def freeze_support(self):
        return None

    def get_all_start_methods(self):
        return _core.get_all_start_methods()

    def get_context(self, method: str | None = None):
        return _core.get_context(method)

    def get_logger(self):
        return _LOGGER

    def get_start_method(self, allow_none: bool = False):
        return _core.get_start_method(allow_none=allow_none)

    def log_to_stderr(self, level=None):
        logger = self.get_logger()
        if level is not None:
            logger.setLevel(level)
        return logger

    def set_executable(self, _path):
        return None

    def set_forkserver_preload(self, _modules):
        return None

    def set_start_method(self, method: str, force: bool = False):
        return _core.set_start_method(method, force=force)


_facade = _TopLevelFacade()

Array = _facade.Array
Barrier = _facade.Barrier
BoundedSemaphore = _facade.BoundedSemaphore
Condition = _facade.Condition
Event = _facade.Event
JoinableQueue = _facade.JoinableQueue
Lock = _facade.Lock
Manager = _facade.Manager
Pipe = _facade.Pipe
Pool = _facade.Pool
Queue = _facade.Queue
RLock = _facade.RLock
RawArray = _facade.RawArray
RawValue = _facade.RawValue
Semaphore = _facade.Semaphore
SimpleQueue = _facade.SimpleQueue
Value = _facade.Value
allow_connection_pickling = _facade.allow_connection_pickling
cpu_count = _facade.cpu_count
freeze_support = _facade.freeze_support
get_all_start_methods = _facade.get_all_start_methods
get_context = _facade.get_context
get_logger = _facade.get_logger
get_start_method = _facade.get_start_method
log_to_stderr = _facade.log_to_stderr
set_executable = _facade.set_executable
set_forkserver_preload = _facade.set_forkserver_preload
set_start_method = _facade.set_start_method

Process = _core.Process
TimeoutError = _builtins.TimeoutError

SUBDEBUG = 5
SUBWARNING = 25

context = _types
process = _types
reduction = _types
reducer = _types
sys = _sys

# Keep underscore-prefixed entry hooks available for multiprocessing.spawn.
_spawn_main = _core._spawn_main
_spawn_trace = _core._spawn_trace

_apply_api_surface(
    "multiprocessing",
    globals(),
    providers={
        "Array": Array,
        "AuthenticationError": AuthenticationError,
        "Barrier": Barrier,
        "BoundedSemaphore": BoundedSemaphore,
        "BufferTooShort": BufferTooShort,
        "Condition": Condition,
        "Event": Event,
        "JoinableQueue": JoinableQueue,
        "Lock": Lock,
        "Manager": Manager,
        "Pipe": Pipe,
        "Pool": Pool,
        "Process": Process,
        "ProcessError": ProcessError,
        "Queue": Queue,
        "RLock": RLock,
        "RawArray": RawArray,
        "RawValue": RawValue,
        "SUBDEBUG": SUBDEBUG,
        "SUBWARNING": SUBWARNING,
        "Semaphore": Semaphore,
        "SimpleQueue": SimpleQueue,
        "TimeoutError": TimeoutError,
        "Value": Value,
        "active_children": active_children,
        "allow_connection_pickling": allow_connection_pickling,
        "context": context,
        "cpu_count": cpu_count,
        "current_process": current_process,
        "freeze_support": freeze_support,
        "get_all_start_methods": get_all_start_methods,
        "get_context": get_context,
        "get_logger": get_logger,
        "get_start_method": get_start_method,
        "log_to_stderr": log_to_stderr,
        "parent_process": parent_process,
        "process": process,
        "reducer": reducer,
        "reduction": reduction,
        "set_executable": set_executable,
        "set_forkserver_preload": set_forkserver_preload,
        "set_start_method": set_start_method,
        "sys": sys,
    },
    prune=True,
)
