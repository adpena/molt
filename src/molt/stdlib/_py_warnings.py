"""Intrinsic-backed compatibility surface for CPython's `_py_warnings`."""


import sys
import warnings as _warnings_mod


WarningMessage = _warnings_mod.WarningMessage
catch_warnings = _warnings_mod.catch_warnings
defaultaction = _warnings_mod.defaultaction
deprecated = _warnings_mod.deprecated
filters = _warnings_mod.filters
filterwarnings = _warnings_mod.filterwarnings
formatwarning = _warnings_mod.formatwarning
onceregistry = _warnings_mod.onceregistry
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
