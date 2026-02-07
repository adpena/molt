"""Minimal gc shim for Molt."""

from __future__ import annotations

# Intrinsic-only stdlib guard.
from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())


def _require_callable_intrinsic(name: str):
    value = _require_intrinsic(name, globals())
    if not callable(value):
        raise RuntimeError(f"intrinsic unavailable: {name}")
    return value


_MOLT_GC_COLLECT = _require_callable_intrinsic("molt_gc_collect")
_MOLT_GC_ENABLE = _require_callable_intrinsic("molt_gc_enable")
_MOLT_GC_DISABLE = _require_callable_intrinsic("molt_gc_disable")
_MOLT_GC_ISENABLED = _require_callable_intrinsic("molt_gc_isenabled")
_MOLT_GC_SET_THRESHOLD = _require_callable_intrinsic("molt_gc_set_threshold")
_MOLT_GC_GET_THRESHOLD = _require_callable_intrinsic("molt_gc_get_threshold")
_MOLT_GC_SET_DEBUG = _require_callable_intrinsic("molt_gc_set_debug")
_MOLT_GC_GET_DEBUG = _require_callable_intrinsic("molt_gc_get_debug")
_MOLT_GC_GET_COUNT = _require_callable_intrinsic("molt_gc_get_count")

garbage: list[object] = []


def collect(generation: int = 2) -> int:
    return int(_MOLT_GC_COLLECT(generation))


def enable() -> None:
    _MOLT_GC_ENABLE()
    return None


def disable() -> None:
    _MOLT_GC_DISABLE()
    return None


def isenabled() -> bool:
    return bool(_MOLT_GC_ISENABLED())


def set_threshold(th0: int, th1: int = 0, th2: int = 0) -> None:
    _MOLT_GC_SET_THRESHOLD(th0, th1, th2)
    return None


def get_threshold() -> tuple[int, int, int]:
    value = _MOLT_GC_GET_THRESHOLD()
    if (
        isinstance(value, (tuple, list))
        and len(value) == 3
        and isinstance(value[0], int)
        and isinstance(value[1], int)
        and isinstance(value[2], int)
    ):
        return int(value[0]), int(value[1]), int(value[2])
    raise RuntimeError("gc get_threshold intrinsic returned invalid value")


def set_debug(flags: int) -> None:
    _MOLT_GC_SET_DEBUG(flags)
    return None


def get_debug() -> int:
    return int(_MOLT_GC_GET_DEBUG())


def get_count() -> tuple[int, int, int]:
    value = _MOLT_GC_GET_COUNT()
    if (
        isinstance(value, (tuple, list))
        and len(value) == 3
        and isinstance(value[0], int)
        and isinstance(value[1], int)
        and isinstance(value[2], int)
    ):
        return int(value[0]), int(value[1]), int(value[2])
    raise RuntimeError("gc get_count intrinsic returned invalid value")


__all__ = [
    "collect",
    "disable",
    "enable",
    "garbage",
    "get_count",
    "get_debug",
    "get_threshold",
    "isenabled",
    "set_debug",
    "set_threshold",
]
