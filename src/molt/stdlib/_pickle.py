"""Intrinsic-backed `_pickle` compatibility surface."""

from __future__ import annotations


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
