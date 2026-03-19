"""Intrinsic-backed `_pickle` compatibility surface."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_STDLIB_PROBE = _require_intrinsic("molt_stdlib_probe")
_MOLT_PICKLE_DUMPS_CORE = _require_intrinsic("molt_pickle_dumps_core")
_MOLT_PICKLE_LOADS_CORE = _require_intrinsic("molt_pickle_loads_core")

from pickle import (  # noqa: E402
    DEFAULT_PROTOCOL,
    HIGHEST_PROTOCOL,
    PickleError,
    PickleBuffer,
    Pickler,
    PicklingError,
    Unpickler,
    UnpicklingError,
    dump,
    dumps,
    load,
    loads,
)

__all__ = [
    "PickleError",
    "PicklingError",
    "UnpicklingError",
    "PickleBuffer",
    "Pickler",
    "Unpickler",
    "DEFAULT_PROTOCOL",
    "HIGHEST_PROTOCOL",
    "dump",
    "dumps",
    "load",
    "loads",
]

del _MOLT_STDLIB_PROBE
del _MOLT_PICKLE_DUMPS_CORE
del _MOLT_PICKLE_LOADS_CORE
globals().pop("_require_intrinsic", None)
