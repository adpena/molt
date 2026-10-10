"""Compatibility surface for CPython `_threading_local`."""


from contextlib import contextmanager
from threading import RLock as _ThreadRLock
from threading import current_thread, local
from weakref import ReferenceType as ref


def RLock(*args, **kwargs):
    return _ThreadRLock(*args, **kwargs)


__all__ = ["RLock", "contextmanager", "current_thread", "local", "ref"]
