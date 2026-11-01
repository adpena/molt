"""Intrinsic-backed `_pickle` compatibility surface.

CPython's C `_pickle` exports the pickler, unpickler, buffer and error types
and the four module functions, without the protocol constants or `__all__`.
"""

from pickle import (  # noqa: E402
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
