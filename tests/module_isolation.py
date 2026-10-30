"""Removing a module from ``sys.modules`` for one test, absent or not.

``monkeypatch.delitem(sys.modules, name, raising=False)`` records nothing when
``name`` is absent, so a module the test then imports under that name outlives
the test and changes what later tests on the same worker import (HF-472: Molt's
``logging.config`` left behind broke Django's logging setup five tests later).
"""

from __future__ import annotations

import importlib.util
import sys
from types import ModuleType

import pytest


def forget_module(patch: pytest.MonkeyPatch, name: str) -> None:
    """Remove ``name`` from ``sys.modules`` until ``patch`` undoes; restore it
    as found, present or absent."""
    patch.setitem(sys.modules, name, None)
    patch.delitem(sys.modules, name)


def private_module_copy(name: str) -> ModuleType:
    """A fresh, unregistered copy of module ``name`` whose globals a test owns.

    Replacing a global of the shared module, such as ``posixpath.os``, changes
    ``os.path`` for the whole process: pytest's own teardown hooks then run
    pathlib against the replacement, raise before fixture teardown and skip the
    finalizers, so the replacement outlives the test (2026-10-30, HF-582's
    landing). A copy runs the same CPython source with globals of its own.
    """
    spec = importlib.util.find_spec(name)
    if spec is None or spec.loader is None:
        raise ModuleNotFoundError(f"no loadable module {name!r}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module
