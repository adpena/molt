"""Removing a module from ``sys.modules`` for one test, absent or not.

``monkeypatch.delitem(sys.modules, name, raising=False)`` records nothing when
``name`` is absent, so a module the test then imports under that name outlives
the test and changes what later tests on the same worker import (HF-472: Molt's
``logging.config`` left behind broke Django's logging setup five tests later).
"""

from __future__ import annotations

import sys

import pytest


def forget_module(patch: pytest.MonkeyPatch, name: str) -> None:
    """Remove ``name`` from ``sys.modules`` until ``patch`` undoes; restore it
    as found, present or absent."""
    patch.setitem(sys.modules, name, None)
    patch.delitem(sys.modules, name)
