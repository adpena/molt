"""Minimal gc shim for Molt."""

from __future__ import annotations

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): implement
# the full gc module API and wire it to the runtime cycle collector once available.

_enabled = True
_thresholds = (0, 0, 0)
_debug_flags = 0
garbage: list[object] = []


def collect(generation: int = 2) -> int:
    del generation
    return 0


def enable() -> None:
    global _enabled
    _enabled = True


def disable() -> None:
    global _enabled
    _enabled = False


def isenabled() -> bool:
    return _enabled


def set_threshold(th0: int, th1: int = 0, th2: int = 0) -> None:
    global _thresholds
    _thresholds = (int(th0), int(th1), int(th2))


def get_threshold() -> tuple[int, int, int]:
    return _thresholds


def set_debug(flags: int) -> None:
    global _debug_flags
    _debug_flags = int(flags)


def get_debug() -> int:
    return _debug_flags


def get_count() -> tuple[int, int, int]:
    return (0, 0, 0)


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
