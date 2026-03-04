from __future__ import annotations

import functools
import time
from collections.abc import Callable
from typing import ParamSpec, TypeVar, cast


_P = ParamSpec("_P")
_R = TypeVar("_R")


def profiled(label: str) -> Callable[[Callable[_P, _R]], Callable[_P, _R]]:
    """Decorator that reports method wall-time to _record_profile_duration()."""

    def _decorate(func: Callable[_P, _R]) -> Callable[_P, _R]:
        @functools.wraps(func)
        def _wrapped(*args: _P.args, **kwargs: _P.kwargs) -> _R:
            started = time.perf_counter()
            try:
                return func(*args, **kwargs)
            finally:
                if args:
                    duration_ms = max((time.perf_counter() - started) * 1000.0, 0.0)
                    owner = args[0]
                    recorder = getattr(owner, "_record_profile_duration", None)
                    if callable(recorder):
                        recorder(label, duration_ms)

        return cast(Callable[_P, _R], _wrapped)

    return _decorate
