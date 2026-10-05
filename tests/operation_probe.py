"""Attribute probes to a synchronous test operation, not other harness threads.

Use only when the measured operation and its callees run on the installing
thread. Operations that dispatch workers must own and instrument those workers
explicitly; this helper does not measure their work.
"""

from __future__ import annotations

from collections.abc import Callable
from functools import wraps
from threading import get_ident
from typing import ParamSpec, TypeVar

_Args = ParamSpec("_Args")
_Result = TypeVar("_Result")


def same_thread_probe(
    original: Callable[_Args, _Result], probe: Callable[_Args, _Result]
) -> Callable[_Args, _Result]:
    owner_thread = get_ident()

    @wraps(original)
    def observed(*args: _Args.args, **kwargs: _Args.kwargs) -> _Result:
        if get_ident() == owner_thread:
            return probe(*args, **kwargs)
        return original(*args, **kwargs)

    return observed
