"""Public API surface shim for ``concurrent.futures._base``."""

from __future__ import annotations

import collections
from collections import namedtuple as _namedtuple
import logging
import threading
import time
import types

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

import concurrent.futures as _futures

ALL_COMPLETED = _futures.ALL_COMPLETED
FIRST_COMPLETED = _futures.FIRST_COMPLETED
FIRST_EXCEPTION = _futures.FIRST_EXCEPTION

PENDING = "PENDING"
RUNNING = "RUNNING"
CANCELLED = "CANCELLED"
CANCELLED_AND_NOTIFIED = "CANCELLED_AND_NOTIFIED"
FINISHED = "FINISHED"

CancelledError = _futures.CancelledError
TimeoutError = _futures.TimeoutError
InvalidStateError = _futures.InvalidStateError
BrokenExecutor = _futures.BrokenExecutor
Future = _futures.Future


class Error(Exception):
    """Base concurrent.futures._base error."""


class Executor:
    def submit(self, fn, /, *args, **kwargs):
        raise NotImplementedError()

    def map(self, fn, *iterables, timeout=None, chunksize=1):
        return _futures.Executor.map(
            self, fn, *iterables, timeout=timeout, chunksize=chunksize
        )

    def shutdown(self, wait=True, *, cancel_futures=False):
        raise NotImplementedError()

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.shutdown(wait=True)
        return False


DoneAndNotDoneFutures = _namedtuple("DoneAndNotDoneFutures", "done not_done")
LOGGER = logging.getLogger("concurrent.futures")


def wait(fs, timeout: float | None = None, return_when: str = ALL_COMPLETED):
    result = _futures.wait(fs, timeout=timeout, return_when=return_when)
    try:
        return DoneAndNotDoneFutures(result.done, result.not_done)
    except Exception:
        done, not_done = result
        return DoneAndNotDoneFutures(done, not_done)


def as_completed(fs, timeout: float | None = None):
    return _futures.as_completed(fs, timeout=timeout)


__all__ = [
    "ALL_COMPLETED",
    "BrokenExecutor",
    "CANCELLED",
    "CANCELLED_AND_NOTIFIED",
    "CancelledError",
    "DoneAndNotDoneFutures",
    "Error",
    "Executor",
    "FINISHED",
    "FIRST_COMPLETED",
    "FIRST_EXCEPTION",
    "Future",
    "InvalidStateError",
    "LOGGER",
    "PENDING",
    "RUNNING",
    "TimeoutError",
    "as_completed",
    "collections",
    "logging",
    "threading",
    "time",
    "types",
    "wait",
]
