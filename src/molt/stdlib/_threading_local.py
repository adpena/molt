"""Compatibility surface for CPython `_threading_local`."""

from _intrinsics import require_intrinsic as _require_intrinsic

from contextlib import contextmanager
from threading import RLock as _ThreadRLock
from threading import current_thread, local
from weakref import ReferenceType as ref

_require_intrinsic("molt_capabilities_has", globals())


def RLock(*args, **kwargs):
    return _ThreadRLock(*args, **kwargs)


__all__ = ["RLock", "contextmanager", "current_thread", "local", "ref"]
