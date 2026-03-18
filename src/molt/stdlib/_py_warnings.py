"""Intrinsic-backed compatibility surface for CPython's `_py_warnings`."""

from _intrinsics import require_intrinsic as _require_intrinsic

import sys
import warnings as _warnings_mod

_require_intrinsic("molt_warnings_warn")

WarningMessage = _warnings_mod._WarningRecord
catch_warnings = _warnings_mod._CatchWarnings
defaultaction = _warnings_mod._default_action
deprecated = _warnings_mod.deprecated
filters = _warnings_mod._filters
filterwarnings = _warnings_mod.filterwarnings
formatwarning = _warnings_mod.formatwarning
onceregistry: dict[object, object] = {}
resetwarnings = _warnings_mod.resetwarnings
showwarning = _warnings_mod.showwarning
simplefilter = _warnings_mod.simplefilter
warn = _warnings_mod.warn
warn_explicit = _warnings_mod.warn_explicit

__all__ = [
    "WarningMessage",
    "catch_warnings",
    "defaultaction",
    "deprecated",
    "filters",
    "filterwarnings",
    "formatwarning",
    "onceregistry",
    "resetwarnings",
    "showwarning",
    "simplefilter",
    "sys",
    "warn",
    "warn_explicit",
]
