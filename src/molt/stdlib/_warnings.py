"""CPython's `_warnings` accelerator: the runtime warnings engine.

`filters`, `_onceregistry` and `_defaultaction` are the engine's policy objects,
which `warnings` aliases as `filters`, `onceregistry` and `defaultaction`.
"""

from warnings import _filters_mutated
from warnings import defaultaction as _defaultaction
from warnings import filters
from warnings import onceregistry as _onceregistry
from warnings import warn, warn_explicit


__all__ = [
    "filters",
    "warn",
    "warn_explicit",
]
