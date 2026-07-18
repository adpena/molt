"""CPython-compatible generational cyclic garbage collector controls."""

from __future__ import annotations

# Intrinsic-only stdlib guard.
from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_STDLIB_PROBE = _require_intrinsic("molt_stdlib_probe")


def _require_callable_intrinsic(name: str):
    value = _require_intrinsic(name)
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
_MOLT_GC_GET_STATS = _require_callable_intrinsic("molt_gc_get_stats")
_MOLT_GC_IS_TRACKED = _require_callable_intrinsic("molt_gc_is_tracked")
_MOLT_GC_IS_FINALIZED = _require_callable_intrinsic("molt_gc_is_finalized")
_MOLT_GC_GET_OBJECTS = _require_callable_intrinsic("molt_gc_get_objects")
_MOLT_GC_GET_REFERENTS = _require_callable_intrinsic("molt_gc_get_referents")
_MOLT_GC_GET_REFERRERS = _require_callable_intrinsic("molt_gc_get_referrers")
_MOLT_GC_CALLBACKS = _require_callable_intrinsic("molt_gc_callbacks")
_MOLT_GC_GARBAGE = _require_callable_intrinsic("molt_gc_garbage")
_MOLT_GC_FREEZE = _require_callable_intrinsic("molt_gc_freeze")
_MOLT_GC_UNFREEZE = _require_callable_intrinsic("molt_gc_unfreeze")
_MOLT_GC_GET_FREEZE_COUNT = _require_callable_intrinsic("molt_gc_get_freeze_count")

DEBUG_STATS = 1
DEBUG_COLLECTABLE = 2
DEBUG_UNCOLLECTABLE = 4
DEBUG_SAVEALL = 32
DEBUG_LEAK = DEBUG_COLLECTABLE | DEBUG_UNCOLLECTABLE | DEBUG_SAVEALL

callbacks: list[object] = _MOLT_GC_CALLBACKS()
garbage: list[object] = _MOLT_GC_GARBAGE()
_MISSING = object()


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


def set_threshold(th0: int, th1: object = _MISSING, th2: object = _MISSING) -> None:
    if th1 is _MISSING or th2 is _MISSING:
        current = get_threshold()
        if th1 is _MISSING:
            th1 = current[1]
        if th2 is _MISSING:
            th2 = current[2]
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


def get_stats() -> list[dict[str, int]]:
    value = _MOLT_GC_GET_STATS()
    if not isinstance(value, list) or len(value) != 3:
        raise RuntimeError("gc get_stats intrinsic returned invalid value")
    for stats in value:
        if not isinstance(stats, dict) or not all(
            isinstance(stats.get(key), int)
            for key in ("collections", "collected", "uncollectable")
        ):
            raise RuntimeError("gc get_stats intrinsic returned invalid value")
    return value


def is_tracked(obj: object) -> bool:
    return bool(_MOLT_GC_IS_TRACKED(obj))


def is_finalized(obj: object) -> bool:
    return bool(_MOLT_GC_IS_FINALIZED(obj))


def get_objects(generation: int | None = None) -> list[object]:
    return _MOLT_GC_GET_OBJECTS(generation)


def get_referents(*objs: object) -> list[object]:
    return _MOLT_GC_GET_REFERENTS(objs)


def get_referrers(*objs: object) -> list[object]:
    return _MOLT_GC_GET_REFERRERS(objs)


def freeze() -> None:
    _MOLT_GC_FREEZE()
    return None


def unfreeze() -> None:
    _MOLT_GC_UNFREEZE()
    return None


def get_freeze_count() -> int:
    return int(_MOLT_GC_GET_FREEZE_COUNT())


__all__ = [
    "DEBUG_COLLECTABLE",
    "DEBUG_LEAK",
    "DEBUG_SAVEALL",
    "DEBUG_STATS",
    "DEBUG_UNCOLLECTABLE",
    "callbacks",
    "collect",
    "disable",
    "enable",
    "garbage",
    "get_count",
    "get_debug",
    "get_freeze_count",
    "get_objects",
    "get_referents",
    "get_referrers",
    "get_stats",
    "get_threshold",
    "freeze",
    "is_finalized",
    "is_tracked",
    "isenabled",
    "set_debug",
    "set_threshold",
    "unfreeze",
]
