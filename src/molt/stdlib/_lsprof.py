"""Low-level profiler used by `cProfile`.

CPython exposes _lsprof as a C extension that backs cProfile.Profile.
In molt's compiled-binary contract there is no runtime profiling
interpreter to hook into — the program is already compiled to native
code. This module provides a deterministic shim that matches the import
surface so `import cProfile` succeeds and `cProfile.Profile()` is
constructable; the resulting profile is honestly empty.

The Profiler class accepts the standard CPython kwargs (timer,
timeunit, subcalls, builtins) and records nothing. enable() and
disable() are no-ops; getstats() returns an empty list. This matches
CPython's behavior when profiling is enabled but the profiled function
returns immediately.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY


class profiler_entry(tuple):
    """Per-function profile entry, mirroring CPython's _lsprof.profiler_entry."""

    __slots__ = ()

    @property
    def code(self):
        return self[0]

    @property
    def callcount(self):
        return self[1]

    @property
    def reccallcount(self):
        return self[2]

    @property
    def totaltime(self):
        return self[3]

    @property
    def inlinetime(self):
        return self[4]

    @property
    def calls(self):
        return self[5]


class profiler_subentry(tuple):
    """Per-subcall profile entry, mirroring CPython's _lsprof.profiler_subentry."""

    __slots__ = ()

    @property
    def code(self):
        return self[0]

    @property
    def callcount(self):
        return self[1]

    @property
    def reccallcount(self):
        return self[2]

    @property
    def totaltime(self):
        return self[3]

    @property
    def inlinetime(self):
        return self[4]


class Profiler:
    """Deterministic-shim profiler.

    molt's compiled-binary contract has no runtime profiling hooks, so
    this profiler records nothing — enable() and disable() are no-ops,
    getstats() returns an empty list. Construct/destruct semantics
    match CPython so consumers (cProfile, profile harnesses, IDE
    plugins) import and instantiate cleanly.
    """

    def __init__(self, timer=None, timeunit=0.0, subcalls=True, builtins=True):
        self.timer = timer
        self.timeunit = timeunit
        self.subcalls = subcalls
        self.builtins = builtins

    def enable(self, subcalls=True, builtins=True):
        return None

    def disable(self):
        return None

    def clear(self):
        return None

    def getstats(self):
        return []


__all__ = ["Profiler", "profiler_entry", "profiler_subentry"]


globals().pop("_require_intrinsic", None)
