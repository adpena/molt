"""unittest.case — re-exported from unittest for Molt.

CPython exposes the TestCase class through both ``unittest`` and
``unittest.case``.  This module provides the latter so that code doing
``from unittest.case import TestCase`` works correctly.
"""

from __future__ import annotations

# Re-export everything from the parent package so that
# ``from unittest.case import TestCase`` and friends work.
from unittest import (
    TestCase,
    SkipTest,
    skip,
    skipIf,
    skipUnless,
    _AssertRaisesContext,
    _AssertRaisesRegexContext,
)

__all__ = [
    "TestCase",
    "SkipTest",
    "skip",
    "skipIf",
    "skipUnless",
    "_AssertRaisesContext",
    "_AssertRaisesRegexContext",
]

# Expose the module-level DONT_ACCEPT_NONE / DONT_ACCEPT_BARE_BOOLEANS flags
# that CPython's unittest.case carries (used by some test frameworks).
DIFF_OMIT_IDENTICAL = True
