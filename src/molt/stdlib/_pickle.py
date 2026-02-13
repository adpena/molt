"""Intrinsic-backed `_pickle` compatibility surface."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())
_require_intrinsic("molt_pickle_dumps_core", globals())
_require_intrinsic("molt_pickle_loads_core", globals())

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
